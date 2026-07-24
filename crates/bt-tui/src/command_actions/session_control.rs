//! Session-control command actions for the TUI.
//!
//! This module owns command handlers that mutate durable session/control state,
//! including operator-command recording, cancel/steer, settings/defaults,
//! approvals, branch/session switching, and session reset bookkeeping. Pure
//! command dispatch and read-only command rendering stay in the parent module.

use super::super::*;

impl ChatApp {
    pub(crate) async fn record_operator_command(
        &mut self,
        raw_input: &str,
        output: String,
    ) -> Result<(), Box<dyn Error>> {
        self.record_operator_command_with_success(raw_input, output, true)
            .await
    }

    pub(crate) async fn record_operator_command_with_success(
        &mut self,
        raw_input: &str,
        output: String,
        success: bool,
    ) -> Result<(), Box<dyn Error>> {
        self.client
            .record_operator_command(
                self.session_id,
                &RecordOperatorCommandRequest {
                    command_type: "slash_command".to_owned(),
                    raw_input: raw_input.to_owned(),
                    output,
                    success,
                },
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn maybe_record_command_feedback(
        &mut self,
        raw_input: Option<&str>,
        output: impl Into<String>,
        success: bool,
    ) -> Result<(), Box<dyn Error>> {
        let output = output.into();
        if let Some(raw_input) = raw_input {
            self.record_operator_command_with_success(raw_input, output.clone(), success)
                .await?;
        }
        self.show_notice(output);
        Ok(())
    }

    pub(crate) async fn cancel_active_turn(
        &mut self,
        raw_input: Option<&str>,
    ) -> Result<(), Box<dyn Error>> {
        if self.pending_send.is_none() && !self.runtime_busy_for_controls() {
            if !self.pending_approvals.is_empty() {
                return self
                    .maybe_record_command_feedback(
                        raw_input,
                        "Waiting on approval. Choose an action below or use /approve.",
                        false,
                    )
                    .await;
            } else if self
                .queue_inspection
                .as_ref()
                .is_some_and(|inspection| inspection.cancel_requested)
            {
                return self
                    .maybe_record_command_feedback(
                        raw_input,
                        "Cancellation already requested.",
                        false,
                    )
                    .await;
            } else {
                return self
                    .maybe_record_command_feedback(raw_input, "No in-flight turn to cancel.", false)
                    .await;
            }
        }

        self.client
            .cancel_session(
                self.session_id,
                &CancelSessionRequest {
                    reason: Some("cancelled from bt-tui".to_owned()),
                },
            )
            .await?;
        self.clear_local_pending_turn_after_cancel();
        self.clear_task_status();
        self.status = "Cancellation requested".to_owned();
        self.maybe_record_command_feedback(
            raw_input,
            "Cancellation requested for the current turn.",
            true,
        )
        .await
    }

    pub(crate) async fn steer_active_turn(
        &mut self,
        message: String,
        raw_input: Option<&str>,
    ) -> Result<(), Box<dyn Error>> {
        if !self.runtime_busy_for_controls() && self.pending_send.is_none() {
            return self
                .maybe_record_command_feedback(
                    raw_input,
                    "Use a normal message when the agent is idle.",
                    false,
                )
                .await;
        }

        self.client
            .steer_session(self.session_id, &SteerSessionRequest { message })
            .await?;
        self.maybe_record_command_feedback(raw_input, "Steering the current turn.", true)
            .await
    }

    pub(crate) async fn apply_session_settings(
        &mut self,
        connection_id: Option<ConnectionId>,
        model_id: Option<String>,
        tool_mode: Option<SessionToolMode>,
        reset_model_to_default: bool,
        raw_input: Option<&str>,
    ) -> Result<(), Box<dyn Error>> {
        if let Some(connection_id) = connection_id.as_ref()
            && !self.connection_exists(connection_id)
        {
            return self
                .maybe_record_command_feedback(
                    raw_input,
                    format!("Unknown connection `{connection_id}`."),
                    false,
                )
                .await;
        }

        let updated = self
            .client
            .update_session(
                self.session_id,
                &UpdateSessionRequest {
                    connection_id,
                    model_id,
                    tool_mode,
                    reset_model_to_default,
                },
            )
            .await?;
        self.apply_session_metadata(updated.session);
        self.maybe_record_command_feedback(
            raw_input,
            format!("Using {} ({})", self.connection_id, self.effective_model()),
            true,
        )
        .await
    }

    pub(crate) async fn persist_default_connection_command(
        &mut self,
        connection_id: ConnectionId,
        raw_input: Option<&str>,
    ) -> Result<(), Box<dyn Error>> {
        if !self.connection_exists(&connection_id) {
            return self
                .maybe_record_command_feedback(
                    raw_input,
                    format!("Unknown connection `{connection_id}`."),
                    false,
                )
                .await;
        }

        persist_default_connection(&connection_id)?;
        let config = BelltowerConfig::load(None)?;
        let model = config
            .connections
            .iter()
            .find(|connection| connection.id == connection_id)
            .map(|connection| connection.default_model.clone())
            .unwrap_or_else(|| "unknown".to_owned());
        self.maybe_record_command_feedback(
            raw_input,
            format!(
                "Saved default connection {} ({}) for future sessions. Current session unchanged.",
                connection_id, model
            ),
            true,
        )
        .await
    }

    pub(crate) fn validate_persisted_default_model(
        &self,
        connection_id: &ConnectionId,
        model_id: &str,
    ) -> std::result::Result<(), String> {
        let Some(inventory) = self
            .connection_models
            .iter()
            .find(|inventory| &inventory.connection_id == connection_id)
        else {
            return Err(format!(
                "No model inventory is loaded for `{connection_id}`. Refresh connection state and try again."
            ));
        };

        if inventory.discovered_source.is_none() {
            return Err(format!(
                "Cannot save a default model for `{connection_id}` until model discovery succeeds."
            ));
        }

        let discoverable = inventory.discoverable_models();
        if discoverable.is_empty() {
            return Err(format!(
                "No discoverable models are currently available for `{connection_id}`."
            ));
        }

        if discoverable.contains(&model_id) {
            return Ok(());
        }

        Err(format!(
            "Default model `{model_id}` is not discoverable for `{connection_id}`; choose one of: {}",
            discoverable.join(", ")
        ))
    }

    pub(crate) async fn persist_default_model_command(
        &mut self,
        connection_id: ConnectionId,
        model_id: String,
        raw_input: Option<&str>,
    ) -> Result<(), Box<dyn Error>> {
        if !self.connection_exists(&connection_id) {
            return self
                .maybe_record_command_feedback(
                    raw_input,
                    format!("Unknown connection `{connection_id}`."),
                    false,
                )
                .await;
        }

        self.refresh_connection_model_inventory(&connection_id)
            .await?;
        if let Err(message) = self.validate_persisted_default_model(&connection_id, &model_id) {
            return self
                .maybe_record_command_feedback(raw_input, message, false)
                .await;
        }

        persist_connection_default_model(&connection_id, &model_id)?;
        self.maybe_record_command_feedback(
            raw_input,
            format!(
                "Saved default model {} for {}. Future sessions will use it; current session unchanged.",
                model_id, connection_id
            ),
            true,
        )
        .await
    }

    pub(crate) async fn persist_defaults_use_command(
        &mut self,
        connection_id: ConnectionId,
        model_id: Option<String>,
        raw_input: Option<&str>,
    ) -> Result<(), Box<dyn Error>> {
        if !self.connection_exists(&connection_id) {
            return self
                .maybe_record_command_feedback(
                    raw_input,
                    format!("Unknown connection `{connection_id}`."),
                    false,
                )
                .await;
        }

        if model_id.is_some() {
            self.refresh_connection_model_inventory(&connection_id)
                .await?;
        }
        if let Some(model_id) = model_id.as_deref()
            && let Err(message) = self.validate_persisted_default_model(&connection_id, model_id)
        {
            return self
                .maybe_record_command_feedback(raw_input, message, false)
                .await;
        }
        persist_default_connection(&connection_id)?;
        if let Some(model_id) = model_id.as_deref() {
            persist_connection_default_model(&connection_id, model_id)?;
        }

        let config = BelltowerConfig::load(None)?;
        let model = config
            .connections
            .iter()
            .find(|connection| connection.id == connection_id)
            .map(|connection| connection.default_model.clone())
            .unwrap_or_else(|| "unknown".to_owned());
        self.maybe_record_command_feedback(
            raw_input,
            format!(
                "Saved defaults connection={} model={} for future sessions. Current session unchanged.",
                connection_id, model
            ),
            true,
        )
        .await
    }

    pub(crate) async fn start_resolve_last_pending(
        &mut self,
        approved: bool,
        scope: ApprovalScope,
        raw_input: Option<&str>,
    ) -> Result<(), Box<dyn Error>> {
        if self.pending_approval.is_some() {
            return self
                .maybe_record_command_feedback(
                    raw_input,
                    "An approval request is already in flight",
                    false,
                )
                .await;
        }

        let Some(pending) = self.pending_approvals.first().cloned() else {
            return self
                .maybe_record_command_feedback(raw_input, "No pending tool call", false)
                .await;
        };
        self.clear_pending_tool(&pending.call_id);
        self.resolving_tools.insert(pending.call_id.clone());

        let decision = if approved {
            ApprovalDecision::Approved {
                decided_at: time::OffsetDateTime::now_utc(),
                decided_by: "bt-tui".to_owned(),
                scope,
                source: bt_core::ApprovalDecisionSource::Human,
            }
        } else {
            ApprovalDecision::Denied {
                decided_at: time::OffsetDateTime::now_utc(),
                decided_by: "bt-tui".to_owned(),
                reason: Some("Denied from interactive TUI".to_owned()),
                scope,
                source: bt_core::ApprovalDecisionSource::Human,
            }
        };
        let client = self.client.clone();
        let session_id = self.session_id;
        let request = ApproveToolRequest {
            call_id: pending.call_id.clone(),
            tool_name: pending.tool_name.clone(),
            scope,
            decision,
        };
        self.pending_approval = Some(tokio::spawn(async move {
            client
                .approve_tool(session_id, &request)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        }));
        self.pending_approval_started_at = Some(Instant::now());
        self.pending_approval_context = Some(PendingApprovalAction {
            pending: PendingToolView {
                call_id: pending.call_id.clone(),
                tool_name: pending.tool_name.clone(),
                detail: self.pending_tool_detail(&pending.call_id),
            },
        });
        self.status = if approved {
            format!(
                "Approving {} {} {}",
                pending.tool_name,
                short_id_string(&pending.call_id.to_string()),
                approval_scope_label(scope)
            )
        } else {
            format!(
                "Denying {} {} {}",
                pending.tool_name,
                short_id_string(&pending.call_id.to_string()),
                approval_scope_label(scope)
            )
        };
        self.rebuild_render_cache();
        self.maybe_record_command_feedback(raw_input, self.status.clone(), true)
            .await
    }

    pub(crate) async fn cycle_branch(&mut self, direction: isize) -> Result<(), Box<dyn Error>> {
        if let Some(task) = self.pending_history_backfill.take() {
            task.abort();
        }
        if self.branches.is_empty() {
            self.refresh_metadata().await?;
        }
        if self.branches.is_empty() {
            self.show_notice("No branches available");
            return Ok(());
        }

        let current_index = self
            .branches
            .iter()
            .position(|branch| branch.branch_id == self.branch_id)
            .unwrap_or(0);
        let len = self.branches.len() as isize;
        let next_index = (current_index as isize + direction).rem_euclid(len) as usize;
        let next_branch = self.branches[next_index].branch_id;
        self.client
            .activate_branch(
                self.session_id,
                next_branch,
                &ActivateBranchRequest {
                    carry_summary: true,
                },
            )
            .await?;
        self.branch_id = next_branch;
        self.refresh_metadata().await?;
        self.refresh().await?;
        self.show_notice(format!(
            "Switched to branch {}",
            short_id_string(&self.branch_id.to_string())
        ));
        Ok(())
    }

    pub(crate) async fn cycle_session(&mut self, direction: isize) -> Result<(), Box<dyn Error>> {
        if self.sessions.is_empty() {
            self.refresh_metadata().await?;
        }
        let matching = matching_sessions(&self.sessions, &self.project_root, &self.connection_id)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        if matching.len() <= 1 {
            self.show_notice("No other matching sessions");
            return Ok(());
        }

        let current_index = matching
            .iter()
            .position(|session| session.session_id == self.session_id)
            .unwrap_or(0);
        let len = matching.len() as isize;
        let next_index = (current_index as isize + direction).rem_euclid(len) as usize;
        let next_session = matching[next_index].clone();
        self.start_resume_session(next_session, None).await
    }

    pub(crate) async fn create_branch(
        &mut self,
        raw_input: Option<&str>,
    ) -> Result<(), Box<dyn Error>> {
        if let Some(task) = self.pending_history_backfill.take() {
            task.abort();
        }
        let previous_branch = self.branch_id;
        let response = self
            .client
            .create_branch(
                self.session_id,
                &CreateBranchRequest {
                    from_branch_id: self.branch_id,
                    from_event_id: None,
                    activate: true,
                    carry_summary: true,
                },
            )
            .await?;
        self.branch_id = response.branch.branch_id;
        self.refresh_metadata().await?;
        self.refresh().await?;
        self.maybe_record_command_feedback(
            raw_input,
            format!(
                "Created branch {} from {}",
                short_id_string(&self.branch_id.to_string()),
                short_id_string(&previous_branch.to_string())
            ),
            true,
        )
        .await
    }

    pub(crate) async fn create_session(
        &mut self,
        raw_input: Option<&str>,
    ) -> Result<(), Box<dyn Error>> {
        if let Some(task) = self.pending_history_backfill.take() {
            task.abort();
        }
        let created = self
            .client
            .create_session(&CreateSessionRequest {
                project_root: self.project_root.clone(),
                connection_id: self.connection_id.clone(),
                model_id: None,
                tool_mode: None,
                display_name: None,
                objective: None,
                budget: None,
                approval_mode: None,
            })
            .await?;
        self.prepare_for_session_switch();
        self.session_id = created.session.session_id;
        self.branch_id = created.branch.branch_id;
        self.refresh_metadata().await?;
        self.refresh().await?;
        self.ensure_event_stream().await?;
        self.sync_transcript_view();
        self.request_scrollback_reset(true);
        if let Some(raw_input) = raw_input {
            self.record_operator_command(
                raw_input,
                format!(
                    "Created session {} on branch {}",
                    short_id_string(&self.session_id.to_string()),
                    short_id_string(&self.branch_id.to_string())
                ),
            )
            .await?;
        }
        Ok(())
    }

    pub(crate) async fn start_resume_session(
        &mut self,
        session: SessionRecord,
        raw_input: Option<String>,
    ) -> Result<(), Box<dyn Error>> {
        if session.session_id == self.session_id {
            return self
                .maybe_record_command_feedback(
                    raw_input.as_deref(),
                    format!("Already in {}", session_title(Some(&session))),
                    false,
                )
                .await;
        }

        if self.pending_session_load.is_some() {
            return self
                .maybe_record_command_feedback(
                    raw_input.as_deref(),
                    "A session switch is already in progress.",
                    false,
                )
                .await;
        }

        if self.has_pending_request() {
            return self
                .maybe_record_command_feedback(
                    raw_input.as_deref(),
                    "Wait for the current request before resuming another session.",
                    false,
                )
                .await;
        }

        if let Some(task) = self.pending_history_backfill.take() {
            task.abort();
        }
        self.stop_event_stream();
        self.pending_tools.clear();
        self.pending_approvals.clear();
        self.pending_inputs.clear();
        self.queue_inspection = None;
        self.queue_refresh_requested = true;
        self.resolving_tools.clear();
        self.pending_shell_command = None;
        self.pending_approval_context = None;
        self.bottom_shell.command_menu_selection = 0;
        self.bottom_shell.command_menu_scroll_top = 0;
        self.bottom_shell.approval_menu_selection = 0;
        self.bottom_shell.question_choice_selection = 0;
        self.composer.input.clear();
        self.composer.cursor = 0;
        self.resume_follow_lock = false;
        self.auto_follow_messages = true;
        self.pending_resume_command_raw_input = raw_input;
        self.status = format!("Loading {}", session_title(Some(&session)));
        self.rendered_message_lines = vec![Line::from("Loading session...")];
        self.refresh_render_metrics();
        self.scroll_to_bottom();
        let client = self.client.clone();
        self.pending_session_load = Some(tokio::spawn(async move {
            load_session_state(client, session).await
        }));
        Ok(())
    }

    pub(crate) async fn apply_loaded_session(
        &mut self,
        loaded: LoadedSessionState,
        action_label: &str,
    ) -> Result<(), Box<dyn Error>> {
        self.prepare_for_session_switch();
        self.session_id = loaded.session.session_id;
        self.project_root = loaded.session.project_root.to_string();
        self.branch_id = loaded.branch_id;
        self.connection_id = loaded.session.connection_id.clone();
        self.connection_model = loaded.connection_model;
        self.messages = loaded.messages;
        self.message_seq_ids = loaded.message_seq_ids;
        self.operator_commands = loaded.operator_commands;
        self.has_more_history_before = loaded.has_more_history_before;
        self.sessions = loaded.sessions;
        self.branches = loaded.branches;
        self.connections = loaded.connections;
        self.connection_models = loaded.connection_models;
        self.last_event_id = loaded.last_event_id;
        self.status = format!("Ready. {} messages loaded.", self.messages.len());
        self.last_refresh = Instant::now();
        self.last_metadata_refresh = Instant::now();
        self.last_queue_refresh = Instant::now() - QUEUE_REFRESH_INTERVAL;
        self.refresh_tool_views();
        self.on_messages_changed();
        self.rebuild_committed_history_for_scrollback_reset();
        self.refresh_queue_cache(true).await?;
        self.request_scrollback_reset(true);
        self.resume_follow_lock = true;
        self.post_resume_layout_sync_pending = true;
        self.ensure_event_stream().await?;
        if let Some(raw_input) = self.pending_resume_command_raw_input.take() {
            self.record_operator_command(
                &raw_input,
                format!("{action_label} {}", session_title(Some(&loaded.session))),
            )
            .await?;
        }
        self.show_notice(format!(
            "{action_label} {}",
            session_title(Some(&loaded.session))
        ));
        Ok(())
    }

    pub(crate) fn scroll_to_bottom(&mut self) {
        self.auto_follow_messages = true;
    }

    pub(crate) fn reset_transcript_view_state(&mut self) {
        self.clear_separator_skip_line = None;
        self.auto_follow_messages = true;
        self.active_turn.live = false;
        self.clear_live_turn_boundary();
        self.clear_live_turn_items();
        self.active_turn.optimistic_user_text = None;
        self.pending_history_entries.clear();
        self.resume_follow_lock = false;
        self.post_resume_layout_sync_pending = false;
        self.pending_hard_clear = false;
        self.pending_visible_clear = false;
        self.pending_scrollback_reset_text = None;
        self.pending_scrollback_reset_lines = None;
        self.active_turn.committed_assistant_text.clear();
        self.active_turn.deferred_streaming_assistant_text.clear();
        self.active_turn.deferred_canonical_assistant_text = None;
    }

    pub(crate) fn prepare_for_session_switch(&mut self) {
        self.reset_event_stream();
        if let Some(task) = self.pending_history_backfill.take() {
            task.abort();
        }
        self.reset_transcript_view_state();
        self.messages.clear();
        self.message_seq_ids.clear();
        self.operator_commands.clear();
        self.has_more_history_before = false;
        self.pending_tools.clear();
        self.pending_approvals.clear();
        self.pending_inputs.clear();
        self.queue_inspection = None;
        self.queue_refresh_requested = true;
        self.resolving_tools.clear();
        while let Some(pending) = self.pending_message_submissions.pop_front() {
            pending.handle.abort();
        }
        self.pending_shell_command = None;
        self.pending_approval_context = None;
        self.cancel_requested = false;
        self.bottom_shell.command_menu_selection = 0;
        self.bottom_shell.command_menu_scroll_top = 0;
        self.bottom_shell.approval_menu_selection = 0;
        self.bottom_shell.question_choice_selection = 0;
        self.composer.input.clear();
        self.composer.cursor = 0;
        self.clear_input_history_browse();
        self.committed_history_entries.clear();
        self.printed_message_ids.clear();
        self.printed_tool_call_ids.clear();
        self.printed_operator_command_keys.clear();
        self.transient_status = None;
        self.transient_status_until = None;
        self.status = "Loading session...".to_owned();
        self.rendered_message_lines = vec![Line::from("Loading session...")];
        self.wrapped_message_lines = vec![Line::from("Loading session...")];
        self.refresh_render_metrics();
        self.scroll_to_bottom();
    }
}
