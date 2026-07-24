// sessions.rs keeps one bounded runtime concern out of `runtime.rs`; cross-crate ownership remains unchanged.
use super::support::*;
use super::*;

impl BelltowerRuntime {
    pub fn create_session(
        &self,
        project_root: Utf8PathBuf,
        connection_id: ConnectionId,
        model_id: Option<String>,
        tool_mode: SessionToolMode,
        display_name: Option<String>,
        objective: Option<String>,
    ) -> Result<(SessionRecord, BranchRecord)> {
        self.ensure_connection_configured(&connection_id)?;

        let session = SessionRecord {
            session_id: SessionId::new(),
            project_root: project_root.clone(),
            connection_id: connection_id.clone(),
            model_id,
            tool_mode,
            settings_revision_id: default_settings_revision_id(),
            created_at: time::OffsetDateTime::now_utc(),
            updated_at: time::OffsetDateTime::now_utc(),
            status: SessionStatus::Active,
            display_name,
            objective,
            parent_session_id: None,
            parent_branch_id: None,
            parent_turn_id: None,
        };
        let branch = BranchRecord {
            branch_id: bt_core::BranchId::new(),
            session_id: session.session_id,
            parent_branch_id: None,
            parent_event_id: None,
            head_event_id: None,
            summary: None,
            created_at: time::OffsetDateTime::now_utc(),
            is_default: true,
        };

        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .create_session(&session, &branch)?;

        Ok((session, branch))
    }

    pub fn configure_session_budget(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        budget: BudgetConfig,
    ) -> Result<i64> {
        let configuration = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::BudgetConfigured { budget },
        );
        let cancellation = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::SessionCancelled {
                reason: "budget_exhausted".to_owned(),
            },
        );
        self.ensure_session_started_for(session_id, branch_id)?;
        self.take_store_append_fault_for_test()?;
        let mut store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        let committed = store.commit_budget_configuration(&configuration, &cancellation)?;
        self.publish_committed_event(configuration, committed.configuration_seq_id);
        if let Some(seq_id) = committed.cancellation_seq_id {
            self.publish_committed_event(cancellation, seq_id);
        }
        drop(store);
        Ok(committed.configuration_seq_id)
    }

    pub fn session_budget(&self, session_id: SessionId) -> Result<Option<SessionBudgetProjection>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_session_budget(session_id)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn checkpoint_session_budget(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        turn_id: TurnId,
    ) -> Result<BudgetEnforcementOutcome> {
        self.checkpoint_session_budget_at(
            session_id,
            branch_id,
            turn_id,
            Some(time::OffsetDateTime::now_utc()),
        )
    }

    pub fn record_active_turn_finished(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        turn_id: TurnId,
        provider: String,
        model: String,
        status: String,
        finish_reason: Option<String>,
        latency_ms: u64,
    ) -> Result<BudgetEnforcementOutcome> {
        self.commit_active_turn_terminal_events(
            session_id,
            branch_id,
            turn_id,
            time::OffsetDateTime::now_utc(),
            vec![self.turn_finished_event(
                session_id,
                branch_id,
                turn_id,
                provider,
                model,
                status,
                finish_reason,
                latency_ms,
            )],
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    fn checkpoint_session_budget_at(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        turn_id: TurnId,
        terminal_at: Option<time::OffsetDateTime>,
    ) -> Result<BudgetEnforcementOutcome> {
        let Some((checkpoint, cancellation)) =
            self.budget_checkpoint_transition_at(session_id, branch_id, turn_id, terminal_at)?
        else {
            return Ok(BudgetEnforcementOutcome::NotConfigured);
        };
        self.take_store_append_fault_for_test()?;
        let mut store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        let committed = store.commit_budget_checkpoint(&checkpoint, &cancellation)?;
        self.publish_committed_event(checkpoint, committed.checkpoint_seq_id);
        if let Some(seq_id) = committed.cancellation_seq_id {
            self.publish_committed_event(cancellation, seq_id);
        }
        drop(store);

        if committed.exhausted {
            Ok(BudgetEnforcementOutcome::Exhausted)
        } else {
            Ok(BudgetEnforcementOutcome::WithinBudget)
        }
    }

    pub(super) fn commit_active_turn_terminal_events(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        turn_id: TurnId,
        terminal_at: time::OffsetDateTime,
        terminal_events: Vec<EventEnvelope>,
    ) -> Result<BudgetEnforcementOutcome> {
        let Some((checkpoint, cancellation)) = self.budget_checkpoint_transition_at(
            session_id,
            branch_id,
            turn_id,
            Some(terminal_at),
        )?
        else {
            self.ensure_session_started_for(session_id, branch_id)?;
            self.take_store_append_fault_for_test()?;
            let mut store = self.store.lock().map_err(|_| {
                bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned())
            })?;
            let seq_ids = store.commit_turn_terminal_transition(
                session_id,
                branch_id,
                turn_id,
                &terminal_events,
            )?;
            for (event, seq_id) in terminal_events.into_iter().zip(seq_ids.iter().copied()) {
                self.publish_committed_event(event, seq_id);
            }
            drop(store);
            return Ok(BudgetEnforcementOutcome::NotConfigured);
        };
        self.take_store_append_fault_for_test()?;
        let mut store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        let committed = store.commit_budget_terminal_transition(
            &checkpoint,
            &cancellation,
            &terminal_events,
        )?;
        self.publish_committed_event(checkpoint, committed.checkpoint_seq_id);
        if let Some(seq_id) = committed.cancellation_seq_id {
            self.publish_committed_event(cancellation, seq_id);
        }
        for (event, seq_id) in terminal_events
            .into_iter()
            .zip(committed.terminal_seq_ids.iter().copied())
        {
            self.publish_committed_event(event, seq_id);
        }
        drop(store);

        if committed.exhausted {
            Ok(BudgetEnforcementOutcome::Exhausted)
        } else {
            Ok(BudgetEnforcementOutcome::WithinBudget)
        }
    }

    fn budget_checkpoint_transition_at(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        turn_id: TurnId,
        terminal_at: Option<time::OffsetDateTime>,
    ) -> Result<Option<(EventEnvelope, EventEnvelope)>> {
        let Some(projection) = self.session_budget(session_id)? else {
            return Ok(None);
        };
        let turn_budget = self.turn_budget_window(session_id, turn_id, &projection, terminal_at)?;
        let elapsed_delta =
            rounded_elapsed_seconds(turn_budget.anchor_time, turn_budget.finished_at);
        let tokens_used = turn_budget.base_tokens_used + turn_budget.delta_usage.total_tokens;
        let turns_used = turn_budget.base_turns_used + u32::from(!turn_budget.already_checkpointed);
        let elapsed_seconds = turn_budget.base_elapsed_seconds + elapsed_delta;
        let cost_used_usd = if turn_budget.delta_cost_is_unknown {
            None
        } else {
            merge_cost_totals(
                turn_budget.base_cost_used_usd,
                turn_budget.delta_cost_used_usd,
            )
        };
        Ok(Some((
            EventEnvelope::new(
                session_id,
                branch_id,
                SpanKind::Agent,
                EventPayload::BudgetCheckpoint {
                    tokens_used,
                    turns_used,
                    elapsed_seconds,
                    cost_used_usd,
                },
            )
            .with_turn_id(turn_id),
            EventEnvelope::new(
                session_id,
                branch_id,
                SpanKind::Agent,
                EventPayload::SessionCancelled {
                    reason: "budget_exhausted".to_owned(),
                },
            ),
        )))
    }

    pub fn spawn_child_session(
        &self,
        parent_session_id: SessionId,
        parent_branch_id: bt_core::BranchId,
        parent_turn_id: Option<TurnId>,
        objective: String,
        display_name: Option<String>,
        connection_id: Option<ConnectionId>,
        model_id: Option<String>,
    ) -> Result<(SessionRecord, BranchRecord)> {
        let (session, branch, _) = self.spawn_child_session_transition(
            parent_session_id,
            parent_branch_id,
            parent_turn_id,
            objective,
            display_name,
            connection_id,
            model_id,
            false,
        )?;
        Ok((session, branch))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn_child_session_with_initial_objective(
        &self,
        parent_session_id: SessionId,
        parent_branch_id: bt_core::BranchId,
        parent_turn_id: Option<TurnId>,
        objective: String,
        display_name: Option<String>,
        connection_id: Option<ConnectionId>,
        model_id: Option<String>,
    ) -> Result<(SessionRecord, BranchRecord, RelatedSessionMessageReceipt)> {
        let (session, branch, receipt) = self.spawn_child_session_transition(
            parent_session_id,
            parent_branch_id,
            parent_turn_id,
            objective,
            display_name,
            connection_id,
            model_id,
            true,
        )?;
        Ok((
            session,
            branch,
            receipt.expect("initial objective requested"),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_child_session_transition(
        &self,
        parent_session_id: SessionId,
        parent_branch_id: bt_core::BranchId,
        parent_turn_id: Option<TurnId>,
        objective: String,
        display_name: Option<String>,
        connection_id: Option<ConnectionId>,
        model_id: Option<String>,
        include_initial_objective: bool,
    ) -> Result<(
        SessionRecord,
        BranchRecord,
        Option<RelatedSessionMessageReceipt>,
    )> {
        self.validate_subagent_spawn(parent_session_id)?;
        let objective = objective.trim().to_owned();
        if objective.is_empty() {
            return Err(bt_core::BelltowerError::Protocol(
                "spawn objective cannot be empty".to_owned(),
            ));
        }

        let parent_session = self
            .load_session(parent_session_id)?
            .ok_or_else(|| bt_core::BelltowerError::InvalidState("session not found".to_owned()))?;
        let parent_branch = self
            .load_branch(parent_session_id, parent_branch_id)?
            .ok_or_else(|| bt_core::BelltowerError::InvalidState("branch not found".to_owned()))?;

        let branch_turn_ids = self
            .events_of_kind(parent_session_id, "turn.started", Some(parent_branch_id))?
            .into_iter()
            .filter_map(|event| match event.payload {
                EventPayload::TurnStarted { turn_id, .. } => Some(turn_id),
                _ => None,
            })
            .collect::<Vec<_>>();
        let latest_branch_turn_id = branch_turn_ids.last().copied();
        let origin_turn_id = match parent_turn_id {
            Some(turn_id) => {
                if branch_turn_ids.contains(&turn_id) {
                    Some(turn_id)
                } else {
                    return Err(bt_core::BelltowerError::InvalidState(
                        "parent turn does not belong to the selected branch".to_owned(),
                    ));
                }
            }
            None => latest_branch_turn_id,
        };

        let child_connection_id =
            connection_id.unwrap_or_else(|| parent_session.connection_id.clone());
        let child_model_id = if let Some(model_id) = model_id {
            Some(model_id)
        } else if child_connection_id == parent_session.connection_id {
            parent_session.model_id.clone()
        } else {
            None
        };
        self.ensure_connection_configured(&child_connection_id)?;

        let child_session = SessionRecord {
            session_id: SessionId::new(),
            project_root: parent_session.project_root.clone(),
            connection_id: child_connection_id.clone(),
            model_id: child_model_id.clone(),
            tool_mode: parent_session.tool_mode,
            settings_revision_id: default_settings_revision_id(),
            created_at: time::OffsetDateTime::now_utc(),
            updated_at: time::OffsetDateTime::now_utc(),
            status: SessionStatus::Active,
            display_name,
            objective: Some(objective.clone()),
            parent_session_id: Some(parent_session.session_id),
            parent_branch_id: Some(parent_branch.branch_id),
            parent_turn_id: origin_turn_id,
        };
        let child_branch = BranchRecord {
            branch_id: bt_core::BranchId::new(),
            session_id: child_session.session_id,
            parent_branch_id: None,
            parent_event_id: None,
            head_event_id: None,
            summary: None,
            created_at: time::OffsetDateTime::now_utc(),
            is_default: true,
        };

        self.ensure_session_started_for(parent_session.session_id, parent_branch.branch_id)?;
        let spawn_requested = apply_turn_id(
            EventEnvelope::new(
                parent_session.session_id,
                parent_branch.branch_id,
                SpanKind::Chain,
                EventPayload::SessionSpawnRequested {
                    child_session_id: child_session.session_id,
                    objective: objective.clone(),
                    connection_id: child_connection_id.to_string(),
                    model_id: child_model_id.clone(),
                },
            ),
            origin_turn_id,
        );
        let child_started = EventEnvelope::new(
            child_session.session_id,
            child_branch.branch_id,
            SpanKind::Session,
            EventPayload::SessionStarted {
                project_root: child_session.project_root.to_string(),
                connection_id: child_connection_id.to_string(),
            },
        );
        let child_handoff = EventEnvelope::new(
            child_session.session_id,
            child_branch.branch_id,
            SpanKind::Chain,
            EventPayload::SessionHandoffRecorded {
                parent_session_id: parent_session.session_id,
                parent_branch_id: parent_branch.branch_id,
                parent_turn_id: origin_turn_id,
                objective: objective.clone(),
                summary: objective.clone(),
            },
        );
        let spawn_completed = apply_turn_id(
            EventEnvelope::new(
                parent_session.session_id,
                parent_branch.branch_id,
                SpanKind::Chain,
                EventPayload::SessionSpawned {
                    child_session_id: child_session.session_id,
                    child_branch_id: child_branch.branch_id,
                    objective: objective.clone(),
                },
            ),
            origin_turn_id,
        );

        self.take_store_append_fault_for_test()?;
        let mut events = vec![
            spawn_requested,
            child_started,
            child_handoff,
            spawn_completed,
        ];
        let initial_message = include_initial_objective
            .then(|| {
                Self::related_session_message_events(
                    parent_session.session_id,
                    parent_branch.branch_id,
                    origin_turn_id,
                    child_session.session_id,
                    child_branch.branch_id,
                    RelatedSessionMessageKind::Instruction,
                    RelatedSessionDeliveryMode::Wake,
                    None,
                    objective.clone(),
                    Vec::new(),
                )
            })
            .transpose()?;
        if let Some((_, sent, received)) = &initial_message {
            events.push(sent.clone());
            events.push(received.clone());
        }
        let mut store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        let seq_ids = if initial_message.is_some() {
            store.commit_child_session_spawn_with_initial_message(
                &child_session,
                &child_branch,
                &events[0],
                &events[1],
                &events[2],
                &events[3],
                &events[4],
                &events[5],
            )?
        } else {
            store.commit_child_session_spawn(
                &child_session,
                &child_branch,
                &events[0],
                &events[1],
                &events[2],
                &events[3],
            )?
        };
        drop(store);
        let receipt =
            initial_message.map(|(message, sent, received)| RelatedSessionMessageReceipt {
                message_id: message.message_id,
                sent_event_id: sent.event_id,
                sent_seq_id: seq_ids[4],
                received_event_id: received.event_id,
                received_seq_id: seq_ids[5],
                status: RelatedSessionMessageStatus::Pending,
            });
        for (event, seq_id) in events.into_iter().zip(seq_ids) {
            self.publish_committed_event(event, seq_id);
        }

        Ok((child_session, child_branch, receipt))
    }

    pub fn update_session_settings(
        &self,
        session_id: SessionId,
        connection_id: Option<ConnectionId>,
        model_id: Option<Option<String>>,
        tool_mode: Option<SessionToolMode>,
    ) -> Result<SessionRecord> {
        let branch = self.default_branch(session_id)?.ok_or_else(|| {
            bt_core::BelltowerError::InvalidState("default branch not found".to_owned())
        })?;
        let target_connection_id = match connection_id.as_ref() {
            Some(connection_id) => connection_id.clone(),
            None => {
                self.load_session(session_id)?
                    .ok_or_else(|| {
                        bt_core::BelltowerError::InvalidState("session not found".to_owned())
                    })?
                    .connection_id
            }
        };
        self.ensure_connection_configured(&target_connection_id)?;
        self.ensure_session_started_for(session_id, branch.branch_id)?;
        self.take_store_append_fault_for_test()?;
        let mut store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        let committed = store.commit_session_settings_update(
            session_id,
            branch.branch_id,
            connection_id,
            model_id,
            tool_mode,
        )?;
        self.publish_committed_event(committed.event, committed.seq_id);
        Ok(committed.session)
    }

    fn ensure_connection_configured(&self, connection_id: &ConnectionId) -> Result<()> {
        if self.connection(connection_id).is_none() {
            return Err(bt_core::BelltowerError::Config(format!(
                "connection `{connection_id}` is not configured"
            )));
        }
        Ok(())
    }

    pub fn update_session_parent(
        &self,
        session_id: SessionId,
        parent_session_id: SessionId,
        parent_branch_id: bt_core::BranchId,
        parent_turn_id: Option<TurnId>,
    ) -> Result<SessionRecord> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .update_session_parent(
                session_id,
                parent_session_id,
                parent_branch_id,
                parent_turn_id,
            )?;
        self.load_session(session_id)?
            .ok_or_else(|| bt_core::BelltowerError::InvalidState("session not found".to_owned()))
    }

    pub fn create_branch(
        &self,
        session_id: SessionId,
        from_branch_id: bt_core::BranchId,
        from_event_id: Option<bt_core::EventId>,
        activate: bool,
        summary: Option<String>,
    ) -> Result<BranchRecord> {
        let store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        let source_branch = store
            .load_branch(session_id, from_branch_id)?
            .ok_or_else(|| {
                bt_core::BelltowerError::InvalidState("source branch not found".to_owned())
            })?;
        let parent_event_id = if let Some(event_id) = from_event_id {
            let event = store.load_event(session_id, event_id)?.ok_or_else(|| {
                bt_core::BelltowerError::NotFound(format!("fork event {event_id}"))
            })?;
            if event.branch_id != source_branch.branch_id {
                return Err(bt_core::BelltowerError::InvalidState(format!(
                    "fork event {event_id} belongs to branch {}, not source branch {}",
                    event.branch_id, source_branch.branch_id
                )));
            }
            Some(event_id)
        } else {
            source_branch.head_event_id
        };
        drop(store);

        let branch = BranchRecord {
            branch_id: bt_core::BranchId::new(),
            session_id,
            parent_branch_id: Some(source_branch.branch_id),
            parent_event_id,
            head_event_id: None,
            summary,
            created_at: time::OffsetDateTime::now_utc(),
            is_default: false,
        };

        let mut store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        store.create_branch(&branch)?;
        if activate {
            store.set_default_branch(session_id, branch.branch_id)?;
        }
        drop(store);

        if let Some(plan) = self.current_plan(session_id, source_branch.branch_id)? {
            self.record_plan_updated(session_id, branch.branch_id, plan.items, None)?;
        }

        let event = EventEnvelope::new(
            session_id,
            branch.branch_id,
            SpanKind::Chain,
            EventPayload::BranchCreated {
                parent_branch_id: branch.parent_branch_id,
                parent_event_id: branch.parent_event_id,
            },
        );
        self.append_event(event)?;

        Ok(BranchRecord {
            is_default: activate,
            ..branch
        })
    }

    pub fn activate_branch(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
    ) -> Result<()> {
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Chain,
            EventPayload::BranchActivated { branch_id },
        );
        self.append_event_with_store_mutation(event, |store, event| {
            store.set_default_branch_with_event(session_id, branch_id, event)
        })?;
        Ok(())
    }

    pub fn record_branch_summary(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        summary: String,
        files_read: Vec<String>,
        files_modified: Vec<String>,
    ) -> Result<i64> {
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Chain,
            EventPayload::BranchSummarized {
                branch_id,
                summary,
                files_read,
                files_modified,
            },
        );
        self.append_event(event)
    }
}
