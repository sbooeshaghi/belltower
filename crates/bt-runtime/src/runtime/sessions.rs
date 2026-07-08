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
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::BudgetConfigured { budget },
        );
        self.append_event(event)
    }

    pub fn session_budget(&self, session_id: SessionId) -> Result<Option<SessionBudgetProjection>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_session_budget(session_id)
    }

    pub fn ensure_session_budget_allows_turn(&self, session_id: SessionId) -> Result<()> {
        let Some(projection) = self.session_budget(session_id)? else {
            return Ok(());
        };
        if budget_exhausted(
            &projection.budget,
            projection.tokens_used,
            projection.turns_used,
            projection.elapsed_seconds,
            projection.cost_used_usd,
        ) {
            return Err(bt_core::BelltowerError::Protocol(
                "session budget exhausted; adjust the session budget before sending more work"
                    .to_owned(),
            ));
        }
        Ok(())
    }

    pub fn checkpoint_session_budget(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        turn_id: TurnId,
    ) -> Result<BudgetEnforcementOutcome> {
        let Some(projection) = self.session_budget(session_id)? else {
            return Ok(BudgetEnforcementOutcome::NotConfigured);
        };
        let turn_budget = self.turn_budget_window(session_id, turn_id, &projection)?;

        let elapsed_delta =
            rounded_elapsed_seconds(turn_budget.anchor_time, turn_budget.finished_at);
        let tokens_used = turn_budget.base_tokens_used + turn_budget.delta_usage.total_tokens;
        let turns_used = turn_budget.base_turns_used + u32::from(!turn_budget.already_checkpointed);
        let elapsed_seconds = turn_budget.base_elapsed_seconds + elapsed_delta;
        let cost_used_usd = merge_cost_totals(
            turn_budget.base_cost_used_usd,
            turn_budget.delta_cost_used_usd,
        );

        let checkpoint = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::BudgetCheckpoint {
                tokens_used,
                max_tokens: projection.budget.max_tokens,
                turns_used,
                max_turns: projection.budget.max_turns,
                elapsed_seconds,
                max_wall_clock_seconds: projection.budget.max_wall_clock_seconds,
                cost_used_usd,
                max_cost_usd: projection.budget.max_cost_usd,
            },
        )
        .with_turn_id(turn_id);
        self.append_event(checkpoint)?;

        let exhausted = budget_exhausted(
            &projection.budget,
            tokens_used,
            turns_used,
            elapsed_seconds,
            cost_used_usd,
        );
        if exhausted {
            self.cancel_session(session_id, branch_id, "budget_exhausted")?;
            Ok(BudgetEnforcementOutcome::Exhausted)
        } else {
            Ok(BudgetEnforcementOutcome::WithinBudget)
        }
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

        let branch_turns = self
            .turn_history(parent_session_id)?
            .into_iter()
            .filter(|turn| turn.branch_id == parent_branch_id)
            .collect::<Vec<_>>();
        let latest_branch_turn_id = branch_turns.last().map(|turn| turn.turn_id);
        let origin_turn_id = match parent_turn_id {
            Some(turn_id) => {
                if branch_turns.iter().any(|turn| turn.turn_id == turn_id) {
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

        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .create_session(&child_session, &child_branch)?;

        self.record_session_spawn_requested(
            parent_session.session_id,
            parent_branch.branch_id,
            child_session.session_id,
            objective.clone(),
            child_connection_id.to_string(),
            child_model_id.clone(),
            origin_turn_id,
        )?;
        self.record_session_handoff(
            child_session.session_id,
            child_branch.branch_id,
            parent_session.session_id,
            parent_branch.branch_id,
            origin_turn_id,
            objective.clone(),
            objective.clone(),
        )?;
        self.record_session_spawned(
            parent_session.session_id,
            parent_branch.branch_id,
            child_session.session_id,
            child_branch.branch_id,
            objective,
            origin_turn_id,
        )?;

        Ok((child_session, child_branch))
    }

    pub fn update_session_settings(
        &self,
        session_id: SessionId,
        connection_id: Option<ConnectionId>,
        model_id: Option<Option<String>>,
        tool_mode: Option<SessionToolMode>,
    ) -> Result<SessionRecord> {
        let session = self
            .load_session(session_id)?
            .ok_or_else(|| bt_core::BelltowerError::InvalidState("session not found".to_owned()))?;
        let next_connection_id = connection_id.unwrap_or_else(|| session.connection_id.clone());
        let next_model_id = model_id.unwrap_or_else(|| session.model_id.clone());
        let next_tool_mode = tool_mode.unwrap_or(session.tool_mode);
        let next_settings_revision_id = session.settings_revision_id.saturating_add(1);
        self.ensure_connection_configured(&next_connection_id)?;

        let branch = self.default_branch(session_id)?.ok_or_else(|| {
            bt_core::BelltowerError::InvalidState("default branch not found".to_owned())
        })?;
        let event = EventEnvelope::new(
            session_id,
            branch.branch_id,
            SpanKind::Session,
            EventPayload::SessionSettingsUpdated {
                settings_revision_id: next_settings_revision_id,
                connection_id: Some(next_connection_id.to_string()),
                model_id: next_model_id.clone(),
                tool_mode: Some(format!("{:?}", next_tool_mode).to_ascii_lowercase()),
            },
        );
        self.append_event(event)?;

        self.load_session(session_id)?
            .ok_or_else(|| bt_core::BelltowerError::InvalidState("session not found".to_owned()))
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
