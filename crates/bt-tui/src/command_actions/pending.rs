//! Background execution for operator slash commands.
//!
//! Network-bound slash commands must not run inline on the TUI event loop:
//! a slow or unreachable server would freeze keystroke handling and
//! rendering. Command dispatch therefore snapshots the inputs a command
//! needs, spawns the client call(s) on a tokio task, and queues a
//! [`PendingCommand`] that the runtime loop polls next to the other
//! background completions. Completion handlers re-apply the state mutations
//! the old inline command code performed, in the same order, and failures
//! surface through `show_error` tagged with the command label.
//!
//! Durable operator-command recording stays intact: tasks record through
//! the same `record_operator_command` protocol operation the inline code
//! used, either inside the task (when the recorded output is computable
//! there) or in the completion handler (session/branch switches whose
//! output depends on post-switch state).

use super::super::*;
use std::future::Future;
use tokio::sync::oneshot;

/// A slash command whose network work runs on a background task.
///
/// Commands fired while another command is still pending are queued: the
/// `VecDeque` on `ChatApp` preserves submission order, and every spawned
/// task waits on the previous command's completion signal before touching
/// the network, so commands execute strictly serially in the order the
/// operator issued them (matching the old inline-dispatch semantics) while
/// the UI stays responsive. Completions are likewise applied front-first.
pub(crate) struct PendingCommand {
    /// Operator-facing label used for status and error reporting, e.g.
    /// `/model` or `branch switch`.
    pub(crate) label: String,
    /// Raw slash-command input when the invocation should surface in
    /// recorded operator-command history; `None` for keybinding-driven
    /// flows, which were never recorded inline either.
    pub(crate) raw_input: Option<String>,
    pub(crate) handle: JoinHandle<std::result::Result<CommandOutcome, String>>,
}

/// Result payload a background command task hands back to the UI task.
///
/// Each variant carries exactly the data its completion handler needs to
/// replay the state mutations the old inline command code performed.
pub(crate) enum CommandOutcome {
    /// All durable recording already happened in the task; nothing to apply.
    Quiet,
    /// Recording (if any) already happened in the task; show a notice.
    Notice(String),
    /// `/status`: refresh the readiness status cache.
    ReadinessStatus { inspection: StatusInspection },
    /// `/models`: refresh backend and connection-model caches.
    ModelInventory {
        backends: Vec<ModelBackendDescriptor>,
        connections: Vec<ConnectionModelInventory>,
        merge: bool,
    },
    /// `/doctor`: refresh readiness and MCP caches together.
    Doctor {
        status: StatusInspection,
        backends: Vec<ModelBackendDescriptor>,
        connections: Vec<ConnectionModelInventory>,
        servers: Vec<McpServerDescriptor>,
        tools: Vec<McpToolDescriptor>,
    },
    /// `/mcp [reload]`: refresh the MCP inventory cache.
    McpInventory {
        servers: Vec<McpServerDescriptor>,
        tools: Vec<McpToolDescriptor>,
    },
    /// `/use`, `/model`, `/mode`, `/connection`: apply the updated session,
    /// then record + notice (output depends on the applied metadata).
    SessionSettings { session: SessionRecord },
    /// `/spawn` success: refresh session metadata.
    ChildSpawned,
    /// `/refresh` and the refresh keybinding: merge the reloaded transcript.
    TranscriptRefreshed {
        page: LoadedTranscriptPage,
        notice: Option<String>,
    },
    /// `/queue clear`: request a queue re-read and report dropped local work.
    QueueCleared { dropped_submissions: usize },
    /// `/new` and the new-session keybinding: switch to the created session.
    SessionCreated {
        session_id: SessionId,
        branch_id: BranchId,
    },
    /// `/branch` and the create-branch keybinding.
    BranchCreated { branch_id: BranchId, notice: String },
    /// Branch cycling keybindings.
    BranchActivated { branch_id: BranchId },
    /// Session cycling keybindings: hand off to the resume flow.
    SessionCycled { session: SessionRecord },
    /// `/cancel` and the cancel keybinding.
    TurnCancelled { notice: String },
    /// `/detach`: recorded in the task; completion flips the exit flags.
    Detached,
    /// `/defaults model|use`: merge the fetched inventory, then notice.
    InventoryMerged {
        connections: Vec<ConnectionModelInventory>,
        notice: String,
    },
}

/// The command head used for labels, e.g. `/queue clear` -> `/queue`.
pub(crate) fn command_label(raw_input: &str) -> String {
    raw_input
        .split_whitespace()
        .next()
        .unwrap_or("command")
        .to_owned()
}

/// Record a slash command into durable operator-command history from a
/// background task. Mirrors `ChatApp::record_operator_command_with_success`.
pub(crate) async fn record_slash_command(
    client: &BelltowerClient,
    session_id: SessionId,
    raw_input: &str,
    output: String,
    success: bool,
) -> std::result::Result<(), String> {
    client
        .record_operator_command(
            session_id,
            &RecordOperatorCommandRequest {
                command_type: "slash_command".to_owned(),
                raw_input: raw_input.to_owned(),
                output,
                success,
            },
        )
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

impl ChatApp {
    /// Queue a command's network work on a background task.
    ///
    /// The future must own everything it needs (cloned client, copied ids
    /// and strings); it is chained behind the previously queued command so
    /// commands execute serially in submission order.
    pub(crate) fn enqueue_pending_command<F>(
        &mut self,
        label: impl Into<String>,
        raw_input: Option<String>,
        work: F,
    ) where
        F: Future<Output = std::result::Result<CommandOutcome, String>> + Send + 'static,
    {
        let (done_tx, done_rx) = oneshot::channel::<()>();
        let previous = self.command_chain_tail.replace(done_rx);
        let handle = tokio::spawn(async move {
            if let Some(previous) = previous {
                // Wait for the previous command to finish (success, failure,
                // or abort all drop its sender) so queued commands never race
                // each other against the server.
                let _ = previous.await;
            }
            let result = work.await;
            let _ = done_tx.send(());
            result
        });
        self.pending_commands.push_back(PendingCommand {
            label: label.into(),
            raw_input,
            handle,
        });
        self.sync_command_task_status();
    }

    /// Non-blocking replacement for `maybe_record_command_feedback`: slash
    /// invocations record the feedback through the pending-command queue and
    /// show the notice on completion; keybinding flows (no raw input) keep
    /// the immediate local notice the inline code showed.
    pub(crate) fn enqueue_command_feedback(
        &mut self,
        raw_input: Option<&str>,
        output: impl Into<String>,
        success: bool,
    ) {
        let output = output.into();
        let Some(raw_input) = raw_input else {
            self.show_notice(output);
            return;
        };
        let client = self.client.clone();
        let session_id = self.session_id;
        let raw = raw_input.to_owned();
        let label = command_label(raw_input);
        self.enqueue_pending_command(label, Some(raw.clone()), async move {
            record_slash_command(&client, session_id, &raw, output.clone(), success).await?;
            Ok(CommandOutcome::Notice(output))
        });
    }

    /// Queue a command whose output was computed locally and only needs to
    /// be recorded into durable history (e.g. `/help`).
    pub(crate) fn enqueue_recorded_command_output(&mut self, raw_input: &str, output: String) {
        let client = self.client.clone();
        let session_id = self.session_id;
        let raw = raw_input.to_owned();
        let label = command_label(raw_input);
        self.enqueue_pending_command(label, Some(raw.clone()), async move {
            record_slash_command(&client, session_id, &raw, output, true).await?;
            Ok(CommandOutcome::Quiet)
        });
    }

    /// Poll queued command tasks from the runtime loop.
    ///
    /// Completions are applied strictly front-first so results land in
    /// submission order; the serial task chain guarantees the front task is
    /// always the first to finish.
    pub(crate) async fn poll_command_completion(&mut self) -> Result<(), Box<dyn Error>> {
        while self
            .pending_commands
            .front()
            .is_some_and(|pending| pending.handle.is_finished())
        {
            let PendingCommand {
                label,
                raw_input,
                handle,
            } = self
                .pending_commands
                .pop_front()
                .expect("pending command exists");
            match handle.await {
                Ok(Ok(outcome)) => {
                    self.apply_command_outcome(outcome, raw_input.as_deref())
                        .await?;
                }
                Ok(Err(error)) => {
                    self.show_error(format!("{label} failed: {error}"));
                }
                Err(error) => {
                    self.show_error(format!("{label} task failed: {error}"));
                }
            }
        }
        self.sync_command_task_status();
        Ok(())
    }

    async fn apply_command_outcome(
        &mut self,
        outcome: CommandOutcome,
        raw_input: Option<&str>,
    ) -> Result<(), Box<dyn Error>> {
        match outcome {
            CommandOutcome::Quiet => {}
            CommandOutcome::Notice(message) => {
                self.show_notice(message);
            }
            CommandOutcome::ReadinessStatus { inspection } => {
                self.connection_status = Some(inspection);
            }
            CommandOutcome::ModelInventory {
                backends,
                connections,
                merge,
            } => {
                self.model_backends = backends;
                if merge {
                    self.merge_connection_model_inventories(connections);
                } else {
                    self.connection_models = connections;
                }
            }
            CommandOutcome::Doctor {
                status,
                backends,
                connections,
                servers,
                tools,
            } => {
                self.connection_status = Some(status);
                self.model_backends = backends;
                self.connection_models = connections;
                self.mcp_servers = servers;
                self.mcp_tools = tools;
            }
            CommandOutcome::McpInventory { servers, tools } => {
                self.mcp_servers = servers;
                self.mcp_tools = tools;
            }
            CommandOutcome::SessionSettings { session } => {
                self.apply_session_metadata(session);
                self.maybe_record_command_feedback(
                    raw_input,
                    format!("Using {} ({})", self.connection_id, self.effective_model()),
                    true,
                )
                .await?;
            }
            CommandOutcome::ChildSpawned => {
                self.refresh_metadata().await?;
            }
            CommandOutcome::TranscriptRefreshed { page, notice } => {
                self.merge_latest_transcript_page(page);
                self.refresh_tool_views();
                self.request_queue_refresh_now();
                if self.should_follow_messages() {
                    self.scroll_to_bottom();
                }
                if !self.has_pending_request() {
                    self.status = format!("Ready. {} messages loaded.", self.messages.len());
                }
                if self.last_event_id.is_none() {
                    self.bootstrap_event_cursor().await?;
                }
                self.last_refresh = Instant::now();
                if self.last_metadata_refresh.elapsed() >= METADATA_REFRESH_INTERVAL {
                    self.refresh_metadata().await?;
                }
                if let Some(notice) = notice {
                    self.show_notice(notice);
                }
            }
            CommandOutcome::QueueCleared {
                dropped_submissions,
            } => {
                self.request_queue_refresh();
                if dropped_submissions > 0 {
                    self.show_notice(format!(
                        "Dropped {dropped_submissions} pending background message submission(s) before clearing the server queue."
                    ));
                }
            }
            CommandOutcome::SessionCreated {
                session_id,
                branch_id,
            } => {
                self.prepare_for_session_switch();
                self.session_id = session_id;
                self.branch_id = branch_id;
                self.refresh_metadata().await?;
                self.refresh().await?;
                self.ensure_event_stream().await?;
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
            }
            CommandOutcome::BranchCreated { branch_id, notice } => {
                self.branch_id = branch_id;
                self.refresh_metadata().await?;
                self.refresh().await?;
                self.show_notice(notice);
            }
            CommandOutcome::BranchActivated { branch_id } => {
                self.branch_id = branch_id;
                self.refresh_metadata().await?;
                self.refresh().await?;
                self.show_notice(format!(
                    "Switched to branch {}",
                    short_id_string(&self.branch_id.to_string())
                ));
            }
            CommandOutcome::SessionCycled { session } => {
                self.start_resume_session(session, None).await?;
            }
            CommandOutcome::TurnCancelled { notice } => {
                self.clear_local_pending_turn_after_cancel();
                self.clear_task_status();
                self.status = "Cancellation requested".to_owned();
                self.show_notice(notice);
            }
            CommandOutcome::Detached => {
                self.detach_requested = true;
                self.should_quit = true;
            }
            CommandOutcome::InventoryMerged {
                connections,
                notice,
            } => {
                self.merge_connection_model_inventories(connections);
                self.show_notice(notice);
            }
        }
        Ok(())
    }

    /// Keep the "Running /model …" task status in sync with the queue front.
    ///
    /// The command status never clobbers a status another flow owns (for
    /// example an in-flight turn's "Working" status): it is only shown when
    /// the status slot is free or still displaying our own text, and only
    /// cleared when our own text is still on screen.
    pub(crate) fn sync_command_task_status(&mut self) {
        match self.pending_commands.front() {
            Some(front) => {
                let detail = format!("{} …", front.label);
                let ours_active = self.command_task_detail.as_deref().is_some_and(|current| {
                    self.task_status
                        .as_ref()
                        .is_some_and(|status| status.detail.as_deref() == Some(current))
                });
                if self.task_status.is_none() || ours_active {
                    self.task_status = Some(TaskStatusState {
                        header: "Running".to_owned(),
                        detail: Some(detail.clone()),
                        show_interrupt_hint: false,
                    });
                    self.command_task_detail = Some(detail);
                }
            }
            None => {
                if let Some(detail) = self.command_task_detail.take()
                    && self.task_status.as_ref().is_some_and(|status| {
                        status.header == "Running" && status.detail.as_deref() == Some(&detail)
                    })
                {
                    self.clear_task_status();
                }
            }
        }
    }
}
