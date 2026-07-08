use super::*;
use tokio::sync::mpsc::error::TryRecvError;

impl ChatApp {
    pub(super) async fn poll_background(&mut self) -> Result<(), Box<dyn Error>> {
        self.poll_event_stream_background().await?;
        if self.pending_session_load.is_none() {
            self.refresh_queue_cache(false).await?;
        }
        self.poll_session_load_completion().await?;
        self.poll_history_backfill_completion().await?;
        self.poll_send_completion().await?;
        self.poll_message_submission_completion().await?;
        self.poll_approval_completion().await?;
        Ok(())
    }

    async fn poll_event_stream_background(&mut self) -> Result<(), Box<dyn Error>> {
        if self.pending_session_load.is_none() {
            self.ensure_event_stream().await?;
        }

        let mut updates = Vec::new();
        let mut disconnected = false;
        if let Some(receiver) = self.stream_updates.as_mut() {
            loop {
                match receiver.try_recv() {
                    Ok(update) => updates.push(update),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }

        for update in updates {
            self.apply_stream_update(update);
        }

        if disconnected {
            self.stop_event_stream();
            self.schedule_stream_retry();
        }

        if self
            .stream_task
            .as_ref()
            .is_some_and(|task| task.is_finished())
        {
            let task = self.stream_task.take().expect("stream task exists");
            let _ = task.await;
            if self.stream_updates.is_none() {
                self.schedule_stream_retry();
            }
        }

        Ok(())
    }

    async fn poll_session_load_completion(&mut self) -> Result<(), Box<dyn Error>> {
        if !self
            .pending_session_load
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            return Ok(());
        }

        let handle = self
            .pending_session_load
            .take()
            .expect("pending session load exists");
        match handle.await {
            Ok(Ok(loaded)) => {
                self.apply_loaded_session(loaded, "Resumed").await?;
            }
            Ok(Err(error)) => {
                self.pending_resume_command_raw_input = None;
                self.show_error(format!("session switch failed: {error}"));
                self.status = format!("Ready. {} messages loaded.", self.messages.len());
                self.sync_transcript_view();
                self.ensure_event_stream().await?;
            }
            Err(error) => {
                self.pending_resume_command_raw_input = None;
                self.show_error(format!("session switch task failed: {error}"));
                self.status = format!("Ready. {} messages loaded.", self.messages.len());
                self.sync_transcript_view();
                self.ensure_event_stream().await?;
            }
        }

        Ok(())
    }

    async fn poll_history_backfill_completion(&mut self) -> Result<(), Box<dyn Error>> {
        if !self
            .pending_history_backfill
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            return Ok(());
        }

        let handle = self
            .pending_history_backfill
            .take()
            .expect("pending history backfill exists");
        match handle.await {
            Ok(Ok(page)) => {
                let added_messages = page.messages.len();
                let added_commands = page.operator_commands.len();
                self.prepend_older_transcript_page(page);
                self.refresh_tool_views();
                self.show_notice(format!(
                    "Loaded older history: {} messages, {} commands.",
                    added_messages, added_commands
                ));
            }
            Ok(Err(error)) => {
                self.show_error(format!("history backfill failed: {error}"));
            }
            Err(error) => {
                self.show_error(format!("history backfill task failed: {error}"));
            }
        }

        Ok(())
    }

    pub(super) async fn poll_send_completion(&mut self) -> Result<(), Box<dyn Error>> {
        if !self
            .pending_send
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            return Ok(());
        }

        let handle = self.pending_send.take().expect("pending send exists");
        self.pending_send_started_at = None;
        let was_shell_command = self.pending_shell_command.take().is_some();

        match handle.await {
            Ok(Ok(PendingSendCompletion::Ack)) => {
                self.status = format!("Ready. {} messages loaded.", self.messages.len());
                if self.last_metadata_refresh.elapsed() >= METADATA_REFRESH_INTERVAL {
                    self.refresh_metadata().await?;
                }
                if was_shell_command {
                    self.active_turn.live = false;
                    self.clear_live_turn_boundary();
                    self.clear_live_turn_items();
                    if let Some(inspection) = self.queue_inspection.as_mut()
                        && matches!(inspection.runtime_state, SessionRuntimeState::Working)
                        && !inspection.cancel_requested
                        && inspection.pending_steer_count == 0
                        && inspection.queued_messages.is_empty()
                        && inspection.pending_approvals.is_empty()
                        && inspection.pending_inputs.is_empty()
                    {
                        inspection.runtime_state = SessionRuntimeState::Idle;
                    }
                    self.request_queue_refresh_now();
                } else if self.cancel_requested {
                    self.cancel_requested = false;
                    self.show_notice("Turn cancelled.");
                } else if !self.active_turn.live && !self.runtime_busy_for_controls() {
                    self.clear_task_status();
                }
                self.sync_transcript_view();
            }
            Ok(Ok(PendingSendCompletion::MessageSubmitted(response))) => {
                self.apply_message_submission_outcome(response, None, true)
                    .await?;
            }
            Ok(Err(error)) => {
                self.active_turn.live = false;
                self.clear_live_turn_items();
                self.cancel_requested = false;
                self.remove_optimistic_user_message();
                self.clear_task_status();
                self.show_error(format!("send failed: {error}"));
                self.refresh().await?;
            }
            Err(error) => {
                self.active_turn.live = false;
                self.clear_live_turn_items();
                self.cancel_requested = false;
                self.remove_optimistic_user_message();
                self.clear_task_status();
                self.show_error(format!("send task failed: {error}"));
                self.refresh().await?;
            }
        }

        Ok(())
    }

    async fn poll_message_submission_completion(&mut self) -> Result<(), Box<dyn Error>> {
        let mut remaining = VecDeque::new();
        while let Some(pending) = self.pending_message_submissions.pop_front() {
            if !pending.handle.is_finished() {
                remaining.push_back(pending);
                continue;
            }

            let PendingMessageSubmission { preview, handle } = pending;
            match handle.await {
                Ok(Ok(response)) => {
                    self.apply_message_submission_outcome(response, Some(&preview), false)
                        .await?;
                }
                Ok(Err(error)) => {
                    self.show_error(format!(
                        "message submission failed for `{preview}`: {error}"
                    ));
                }
                Err(error) => {
                    self.show_error(format!(
                        "message submission task failed for `{preview}`: {error}"
                    ));
                }
            }
        }
        self.pending_message_submissions = remaining;
        Ok(())
    }

    pub(super) async fn apply_message_submission_outcome(
        &mut self,
        response: SendMessageResponse,
        preview: Option<&str>,
        optimistic: bool,
    ) -> Result<(), Box<dyn Error>> {
        self.request_queue_refresh();
        self.status = format!("Ready. {} messages loaded.", self.messages.len());
        if self.last_metadata_refresh.elapsed() >= METADATA_REFRESH_INTERVAL {
            self.refresh_metadata().await?;
        }

        match response.outcome {
            SendMessageOutcome::Dispatched => {
                if self.cancel_requested {
                    self.cancel_requested = false;
                    self.show_notice("Turn cancelled.");
                } else if let Some(preview) = preview {
                    self.show_notice(format!(
                        "Submitted `{preview}` and the server started it immediately."
                    ));
                }
            }
            SendMessageOutcome::Queued { position } => {
                if optimistic {
                    self.active_turn.live = false;
                    self.clear_live_turn_boundary();
                    self.clear_live_turn_items();
                    self.remove_optimistic_user_message();
                    self.clear_task_status();
                }
                let _ = preview;
                let _ = position;
            }
        }

        self.sync_transcript_view();
        Ok(())
    }

    async fn poll_approval_completion(&mut self) -> Result<(), Box<dyn Error>> {
        if !self
            .pending_approval
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            return Ok(());
        }

        let handle = self
            .pending_approval
            .take()
            .expect("pending approval exists");
        self.pending_approval_started_at = None;
        let action = self.pending_approval_context.take();

        match handle.await {
            Ok(Ok(())) => {
                self.request_queue_refresh();
                if self.last_metadata_refresh.elapsed() >= METADATA_REFRESH_INTERVAL {
                    self.refresh_metadata().await?;
                }
                self.sync_transcript_view();
            }
            Ok(Err(error)) => {
                self.active_turn.live = false;
                self.clear_live_turn_items();
                self.restore_pending_tool_after_failed_approval(action.as_ref());
                self.clear_task_status();
                self.show_error(format!("approval failed: {error}"));
                self.sync_transcript_view();
            }
            Err(error) => {
                self.active_turn.live = false;
                self.clear_live_turn_items();
                self.restore_pending_tool_after_failed_approval(action.as_ref());
                self.clear_task_status();
                self.show_error(format!("approval task failed: {error}"));
                self.sync_transcript_view();
            }
        }

        Ok(())
    }

    fn restore_pending_tool_after_failed_approval(
        &mut self,
        action: Option<&PendingApprovalAction>,
    ) {
        if let Some(action) = action {
            self.resolving_tools.remove(&action.pending.call_id);
            self.upsert_pending_tool(
                action.pending.call_id.clone(),
                action.pending.tool_name.clone(),
                action.pending.detail.clone(),
            );
        }
    }
}
