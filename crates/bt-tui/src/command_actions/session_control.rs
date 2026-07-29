//! Session-control command actions for the TUI.
//!
//! This module owns command handlers that mutate durable session/control state,
//! including operator-command recording, cancel/steer, settings/defaults,
//! approvals, branch/session switching, and session reset bookkeeping. Pure
//! command dispatch and read-only command rendering stay in the parent module.
//! Network-bound handlers enqueue their client calls through the
//! pending-command queue (see `pending`) so they never block the event loop.

use super::super::*;

/// Validate a persisted default model against a connection's inventory.
///
/// Standalone so background command tasks can validate against a freshly
/// fetched inventory without borrowing `ChatApp`.
pub(crate) fn validate_default_model(
    inventories: &[ConnectionModelInventory],
    connection_id: &ConnectionId,
    model_id: &str,
) -> std::result::Result<(), String> {
    let Some(inventory) = inventories
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
                    branch_id: self.branch_id,
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

    pub(crate) fn cancel_active_turn(&mut self, raw_input: Option<&str>) {
        if self.pending_send.is_none() && !self.runtime_busy_for_controls() {
            if !self.pending_approvals.is_empty() {
                self.enqueue_command_feedback(
                    raw_input,
                    "Waiting on approval. Choose an action below or use /approve.",
                    false,
                );
            } else if self
                .queue_inspection
                .as_ref()
                .is_some_and(|inspection| inspection.cancel_requested)
            {
                self.enqueue_command_feedback(raw_input, "Cancellation already requested.", false);
            } else {
                self.enqueue_command_feedback(raw_input, "No in-flight turn to cancel.", false);
            }
            return;
        }

        let client = self.client.clone();
        let session_id = self.session_id;
        let branch_id = self.branch_id;
        let raw = raw_input.map(str::to_owned);
        let label = raw_input.map_or_else(|| "cancel".to_owned(), command_label);
        self.enqueue_pending_command(label, raw.clone(), async move {
            client
                .cancel_session(
                    session_id,
                    &CancelSessionRequest {
                        branch_id,
                        reason: Some("cancelled from bt-tui".to_owned()),
                    },
                )
                .await
                .map_err(|error| error.to_string())?;
            let notice = "Cancellation requested for the current turn.".to_owned();
            if let Some(raw) = raw.as_deref() {
                record_slash_command(&client, session_id, branch_id, raw, notice.clone(), true)
                    .await?;
            }
            Ok(CommandOutcome::TurnCancelled { notice })
        });
    }

    pub(crate) fn steer_active_turn(&mut self, message: String, raw_input: Option<&str>) {
        if !self.runtime_busy_for_controls() && self.pending_send.is_none() {
            self.enqueue_command_feedback(
                raw_input,
                "Use a normal message when the agent is idle.",
                false,
            );
            return;
        }

        let client = self.client.clone();
        let session_id = self.session_id;
        let branch_id = self.branch_id;
        let raw = raw_input.map(str::to_owned);
        let label = raw_input.map_or_else(|| "steer".to_owned(), command_label);
        self.enqueue_pending_command(label, raw.clone(), async move {
            client
                .steer_session(session_id, &SteerSessionRequest { branch_id, message })
                .await
                .map_err(|error| error.to_string())?;
            let notice = "Steering the current turn.".to_owned();
            if let Some(raw) = raw.as_deref() {
                record_slash_command(&client, session_id, branch_id, raw, notice.clone(), true)
                    .await?;
            }
            Ok(CommandOutcome::Notice(notice))
        });
    }

    pub(crate) fn apply_session_settings(
        &mut self,
        connection_id: Option<ConnectionId>,
        model_id: Option<String>,
        tool_mode: Option<SessionToolMode>,
        reset_model_to_default: bool,
        raw_input: Option<&str>,
    ) {
        if let Some(connection_id) = connection_id.as_ref()
            && !self.connection_exists(connection_id)
        {
            self.enqueue_command_feedback(
                raw_input,
                format!("Unknown connection `{connection_id}`."),
                false,
            );
            return;
        }

        let client = self.client.clone();
        let session_id = self.session_id;
        let branch_id = self.branch_id;
        let request = UpdateSessionRequest {
            branch_id,
            connection_id,
            model_id,
            tool_mode,
            reset_model_to_default,
        };
        let label = raw_input.map_or_else(|| "session settings".to_owned(), command_label);
        self.enqueue_pending_command(label, raw_input.map(str::to_owned), async move {
            let updated = client
                .update_session(session_id, &request)
                .await
                .map_err(|error| error.to_string())?;
            Ok(CommandOutcome::SessionSettings {
                session: updated.session,
            })
        });
    }

    pub(crate) fn persist_default_connection_command(
        &mut self,
        connection_id: ConnectionId,
        raw_input: Option<&str>,
    ) {
        if !self.connection_exists(&connection_id) {
            self.enqueue_command_feedback(
                raw_input,
                format!("Unknown connection `{connection_id}`."),
                false,
            );
            return;
        }

        let client = self.client.clone();
        let session_id = self.session_id;
        let branch_id = self.branch_id;
        let raw = raw_input.map(str::to_owned);
        let label = raw_input.map_or_else(|| "defaults".to_owned(), command_label);
        self.enqueue_pending_command(label, raw.clone(), async move {
            persist_default_connection(&connection_id).map_err(|error| error.to_string())?;
            let config = BelltowerConfig::load(None).map_err(|error| error.to_string())?;
            let model = config
                .connections
                .iter()
                .find(|connection| connection.id == connection_id)
                .map(|connection| connection.default_model.clone())
                .unwrap_or_else(|| "unknown".to_owned());
            let message = format!(
                "Saved default connection {} ({}) for future sessions. Current session unchanged.",
                connection_id, model
            );
            if let Some(raw) = raw.as_deref() {
                record_slash_command(&client, session_id, branch_id, raw, message.clone(), true)
                    .await?;
            }
            Ok(CommandOutcome::Notice(message))
        });
    }

    #[cfg(test)]
    pub(crate) fn validate_persisted_default_model(
        &self,
        connection_id: &ConnectionId,
        model_id: &str,
    ) -> std::result::Result<(), String> {
        validate_default_model(&self.connection_models, connection_id, model_id)
    }

    pub(crate) fn persist_default_model_command(
        &mut self,
        connection_id: ConnectionId,
        model_id: String,
        raw_input: Option<&str>,
    ) {
        if !self.connection_exists(&connection_id) {
            self.enqueue_command_feedback(
                raw_input,
                format!("Unknown connection `{connection_id}`."),
                false,
            );
            return;
        }

        let client = self.client.clone();
        let session_id = self.session_id;
        let branch_id = self.branch_id;
        let raw = raw_input.map(str::to_owned);
        let label = raw_input.map_or_else(|| "defaults".to_owned(), command_label);
        self.enqueue_pending_command(label, raw.clone(), async move {
            let fetched = client
                .connection_model_inventory(&connection_id)
                .await
                .map_err(|error| error.to_string())?;
            let inventories = fetched.connections;
            match validate_default_model(&inventories, &connection_id, &model_id) {
                Err(message) => {
                    if let Some(raw) = raw.as_deref() {
                        record_slash_command(
                            &client,
                            session_id,
                            branch_id,
                            raw,
                            message.clone(),
                            false,
                        )
                            .await?;
                    }
                    Ok(CommandOutcome::InventoryMerged {
                        connections: inventories,
                        notice: message,
                    })
                }
                Ok(()) => {
                    persist_connection_default_model(&connection_id, &model_id)
                        .map_err(|error| error.to_string())?;
                    let message = format!(
                        "Saved default model {} for {}. Future sessions will use it; current session unchanged.",
                        model_id, connection_id
                    );
                    if let Some(raw) = raw.as_deref() {
                        record_slash_command(
                            &client,
                            session_id,
                            branch_id,
                            raw,
                            message.clone(),
                            true,
                        )
                            .await?;
                    }
                    Ok(CommandOutcome::InventoryMerged {
                        connections: inventories,
                        notice: message,
                    })
                }
            }
        });
    }

    pub(crate) fn persist_defaults_use_command(
        &mut self,
        connection_id: ConnectionId,
        model_id: Option<String>,
        raw_input: Option<&str>,
    ) {
        if !self.connection_exists(&connection_id) {
            self.enqueue_command_feedback(
                raw_input,
                format!("Unknown connection `{connection_id}`."),
                false,
            );
            return;
        }

        let client = self.client.clone();
        let session_id = self.session_id;
        let branch_id = self.branch_id;
        let raw = raw_input.map(str::to_owned);
        let label = raw_input.map_or_else(|| "defaults".to_owned(), command_label);
        self.enqueue_pending_command(label, raw.clone(), async move {
            let inventories = if let Some(model_id) = model_id.as_deref() {
                let fetched = client
                    .connection_model_inventory(&connection_id)
                    .await
                    .map_err(|error| error.to_string())?;
                if let Err(message) =
                    validate_default_model(&fetched.connections, &connection_id, model_id)
                {
                    if let Some(raw) = raw.as_deref() {
                        record_slash_command(
                            &client,
                            session_id,
                            branch_id,
                            raw,
                            message.clone(),
                            false,
                        )
                            .await?;
                    }
                    return Ok(CommandOutcome::InventoryMerged {
                        connections: fetched.connections,
                        notice: message,
                    });
                }
                Some(fetched.connections)
            } else {
                None
            };
            persist_default_connection(&connection_id).map_err(|error| error.to_string())?;
            if let Some(model_id) = model_id.as_deref() {
                persist_connection_default_model(&connection_id, model_id)
                    .map_err(|error| error.to_string())?;
            }

            let config = BelltowerConfig::load(None).map_err(|error| error.to_string())?;
            let model = config
                .connections
                .iter()
                .find(|connection| connection.id == connection_id)
                .map(|connection| connection.default_model.clone())
                .unwrap_or_else(|| "unknown".to_owned());
            let message = format!(
                "Saved defaults connection={} model={} for future sessions. Current session unchanged.",
                connection_id, model
            );
            if let Some(raw) = raw.as_deref() {
                record_slash_command(&client, session_id, branch_id, raw, message.clone(), true)
                    .await?;
            }
            match inventories {
                Some(connections) => Ok(CommandOutcome::InventoryMerged {
                    connections,
                    notice: message,
                }),
                None => Ok(CommandOutcome::Notice(message)),
            }
        });
    }

    pub(crate) async fn start_resolve_last_pending(
        &mut self,
        approved: bool,
        scope: ApprovalScope,
        raw_input: Option<&str>,
    ) -> Result<(), Box<dyn Error>> {
        if self.pending_approval.is_some() {
            self.enqueue_command_feedback(
                raw_input,
                "An approval request is already in flight",
                false,
            );
            return Ok(());
        }

        let Some(pending) = self.pending_approvals.first().cloned() else {
            self.enqueue_command_feedback(raw_input, "No pending tool call", false);
            return Ok(());
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
        let feedback = self.status.clone();
        self.enqueue_command_feedback(raw_input, feedback, true);
        Ok(())
    }

    pub(crate) fn cycle_branch(&mut self, direction: isize) {
        if let Some(task) = self.pending_history_backfill.take() {
            task.abort();
        }
        let client = self.client.clone();
        let session_id = self.session_id;
        let known_branches = self.branches.clone();
        let current_branch = self.branch_id;
        self.enqueue_pending_command("branch switch", None, async move {
            let branches = if known_branches.is_empty() {
                client
                    .branches(session_id)
                    .await
                    .map_err(|error| error.to_string())?
                    .branches
            } else {
                known_branches
            };
            if branches.is_empty() {
                return Ok(CommandOutcome::Notice("No branches available".to_owned()));
            }

            let current_index = branches
                .iter()
                .position(|branch| branch.branch_id == current_branch)
                .unwrap_or(0);
            let len = branches.len() as isize;
            let next_index = (current_index as isize + direction).rem_euclid(len) as usize;
            let next_branch = branches[next_index].branch_id;
            client
                .activate_branch(
                    session_id,
                    next_branch,
                    &ActivateBranchRequest {
                        carry_summary: true,
                    },
                )
                .await
                .map_err(|error| error.to_string())?;
            Ok(CommandOutcome::BranchActivated {
                branch_id: next_branch,
            })
        });
    }

    pub(crate) fn cycle_session(&mut self, direction: isize) {
        let client = self.client.clone();
        let known_sessions = self.sessions.clone();
        let project_root = self.project_root.clone();
        let connection_id = self.connection_id.clone();
        let current_session = self.session_id;
        self.enqueue_pending_command("session switch", None, async move {
            let sessions = if known_sessions.is_empty() {
                let listed = client
                    .list_sessions()
                    .await
                    .map_err(|error| error.to_string())?
                    .sessions;
                sorted_sessions(&listed)
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                known_sessions
            };
            let matching = matching_sessions(&sessions, &project_root, &connection_id)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            if matching.len() <= 1 {
                return Ok(CommandOutcome::Notice(
                    "No other matching sessions".to_owned(),
                ));
            }

            let current_index = matching
                .iter()
                .position(|session| session.session_id == current_session)
                .unwrap_or(0);
            let len = matching.len() as isize;
            let next_index = (current_index as isize + direction).rem_euclid(len) as usize;
            Ok(CommandOutcome::SessionCycled {
                session: matching[next_index].clone(),
            })
        });
    }

    pub(crate) fn create_branch(&mut self, raw_input: Option<&str>) {
        if let Some(task) = self.pending_history_backfill.take() {
            task.abort();
        }
        let previous_branch = self.branch_id;
        let client = self.client.clone();
        let session_id = self.session_id;
        let raw = raw_input.map(str::to_owned);
        let label = raw_input.map_or_else(|| "branch creation".to_owned(), command_label);
        self.enqueue_pending_command(label, raw.clone(), async move {
            let response = client
                .create_branch(
                    session_id,
                    &CreateBranchRequest {
                        from_branch_id: previous_branch,
                        from_event_id: None,
                        activate: true,
                        carry_summary: true,
                    },
                )
                .await
                .map_err(|error| error.to_string())?;
            let branch_id = response.branch.branch_id;
            let notice = format!(
                "Created branch {} from {}",
                short_id_string(&branch_id.to_string()),
                short_id_string(&previous_branch.to_string())
            );
            if let Some(raw) = raw.as_deref() {
                record_slash_command(&client, session_id, branch_id, raw, notice.clone(), true)
                    .await?;
            }
            Ok(CommandOutcome::BranchCreated { branch_id, notice })
        });
    }

    pub(crate) fn create_session(&mut self, raw_input: Option<&str>) {
        if let Some(task) = self.pending_history_backfill.take() {
            task.abort();
        }
        let client = self.client.clone();
        let request = CreateSessionRequest {
            project_root: self.project_root.clone(),
            connection_id: self.connection_id.clone(),
            model_id: None,
            tool_mode: None,
            display_name: None,
            objective: None,
            budget: None,
            approval_mode: None,
        };
        let label = raw_input.map_or_else(|| "session creation".to_owned(), command_label);
        self.enqueue_pending_command(label, raw_input.map(str::to_owned), async move {
            let created = client
                .create_session(&request)
                .await
                .map_err(|error| error.to_string())?;
            Ok(CommandOutcome::SessionCreated {
                session_id: created.session.session_id,
                branch_id: created.branch.branch_id,
            })
        });
    }

    pub(crate) async fn start_resume_session(
        &mut self,
        session: SessionRecord,
        raw_input: Option<String>,
    ) -> Result<(), Box<dyn Error>> {
        if session.session_id == self.session_id {
            self.enqueue_command_feedback(
                raw_input.as_deref(),
                format!("Already in {}", session_title(Some(&session))),
                false,
            );
            return Ok(());
        }

        if self.pending_session_load.is_some() {
            self.enqueue_command_feedback(
                raw_input.as_deref(),
                "A session switch is already in progress.",
                false,
            );
            return Ok(());
        }

        if self.has_pending_request() || !self.pending_commands.is_empty() {
            self.enqueue_command_feedback(
                raw_input.as_deref(),
                "Wait for the current request before resuming another session.",
                false,
            );
            return Ok(());
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
        // Queued slash commands captured the previous session's identifiers;
        // applying their results after a switch would mutate the wrong
        // session, so drop them like pending message submissions.
        let mut dropped_commands = 0usize;
        while let Some(pending) = self.pending_commands.pop_front() {
            pending.handle.abort();
            dropped_commands += 1;
        }
        self.command_chain_tail = None;
        self.sync_command_task_status();
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
        self.scroll_to_bottom();
        if dropped_commands > 0 {
            self.show_notice(format!(
                "Dropped {dropped_commands} queued slash command(s) during the session switch."
            ));
        }
    }
}
