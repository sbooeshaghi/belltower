use super::*;
use crate::streaming::commit_tick::{CommitTickScope, run_commit_tick};
use std::path::Path;

fn render_history_entries(entries: &[HistoryEntry], width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for entry in entries {
        if !lines.is_empty() {
            match entry.separator {
                ScrollbackSeparator::Paragraph => lines.push(Line::from("")),
                ScrollbackSeparator::Line => {}
                ScrollbackSeparator::None => {}
            }
        }
        lines.extend(entry.cell.display_lines(width));
    }
    lines
}

fn is_ask_tool(tool_name: &str) -> bool {
    tool_name == "ask"
}

impl ChatApp {
    pub(super) fn run_stream_commit_tick(&mut self) {
        if !self.active_turn.cells.is_empty() {
            return;
        }
        let now = Instant::now();
        let outcome = run_commit_tick(
            &mut self.adaptive_chunking,
            self.active_turn.stream_controller.as_mut(),
            CommitTickScope::AnyMode,
            now,
        );
        for cell in outcome.cells {
            self.queue_history_cell(cell, TranscriptEntryKind::Assistant, None);
        }
        let _ = outcome;
    }

    pub(super) fn take_new_history_lines(&mut self) -> Option<Vec<Line<'static>>> {
        if self.pending_history_entries.is_empty() {
            return None;
        }
        let drained = self.pending_history_entries.drain(..).collect::<Vec<_>>();
        Some(render_history_entries(
            &drained,
            self.transcript_output_width.max(1),
        ))
    }

    fn flush_answer_stream(&mut self) -> bool {
        let Some(mut controller) = self.active_turn.stream_controller.take() else {
            return false;
        };
        let raw_text = controller.raw_text().to_owned();
        let emitted = controller.finalize(self.show_reasoning, self.transcript_density());
        self.adaptive_chunking.reset();
        if !raw_text.is_empty() {
            self.active_turn
                .committed_assistant_text
                .push_str(&raw_text);
        }
        let mut flushed = if let Some(cell) = emitted {
            self.queue_history_cell(cell, TranscriptEntryKind::Assistant, None)
        } else {
            false
        };
        if !raw_text.is_empty() {
            flushed |= self.consolidate_assistant_stream_history(&raw_text);
        }
        flushed
    }

    fn start_stream_controller_if_needed(&mut self) {
        if self.active_turn.stream_controller.is_none() {
            self.active_turn.stream_controller = Some(StreamController::new(
                self.transcript_output_width.max(1),
                Path::new(&self.project_root),
            ));
        }
    }

    fn flush_ready_tool_cells(&mut self) -> bool {
        let mut flushed = false;
        while matches!(
            self.active_turn.cells.front(),
            Some(cell) if cell.is_ready_to_flush()
        ) {
            flushed |= self.flush_active_cell(TranscriptEntryKind::ToolCall);
        }
        flushed
    }

    fn has_deferred_assistant_output(&self) -> bool {
        !self
            .active_turn
            .deferred_streaming_assistant_text
            .is_empty()
            || self
                .active_turn
                .deferred_canonical_assistant_text
                .as_deref()
                .is_some_and(|text| !text.is_empty())
    }

    fn flush_complete_tool_cells_for_deferred_assistant(&mut self) -> bool {
        if !self.has_deferred_assistant_output() {
            return false;
        }

        let mut flushed = false;
        while matches!(self.active_turn.cells.front(), Some(cell) if cell.is_complete()) {
            flushed |= self.flush_active_cell(TranscriptEntryKind::ToolCall);
        }
        flushed
    }

    fn flush_complete_active_cells(&mut self) -> bool {
        let mut flushed = false;
        while matches!(self.active_turn.cells.front(), Some(cell) if cell.is_complete()) {
            flushed |= self.flush_active_cell(TranscriptEntryKind::ToolCall);
        }
        flushed
    }

    fn push_streaming_assistant_text_now(
        &mut self,
        text: &str,
        show_reasoning: bool,
        transcript_density: TranscriptDensity,
    ) -> bool {
        if text.is_empty() {
            return false;
        }
        if self.active_turn.stream_controller.is_none() {
            let _ = self.emit_final_work_separator_if_needed();
        }
        self.start_stream_controller_if_needed();
        if let Some(controller) = self.active_turn.stream_controller.as_mut() {
            let _ = controller.push(text, show_reasoning, transcript_density);
            return true;
        }
        false
    }

    fn flush_deferred_streaming_assistant_text(&mut self) -> bool {
        if !self.active_turn.cells.is_empty() {
            return false;
        }
        if self
            .active_turn
            .deferred_streaming_assistant_text
            .is_empty()
        {
            return false;
        }
        let deferred = std::mem::take(&mut self.active_turn.deferred_streaming_assistant_text);
        self.push_streaming_assistant_text_now(
            &deferred,
            self.show_reasoning,
            self.transcript_density(),
        )
    }

    fn drain_tool_frontier_and_resume_assistant_stream(&mut self) -> bool {
        let mut flushed = self.flush_ready_tool_cells();
        flushed |= self.flush_complete_tool_cells_for_deferred_assistant();
        let resumed = if self.active_turn.cells.is_empty() {
            self.flush_deferred_streaming_assistant_text()
        } else {
            false
        };
        flushed || resumed
    }

    fn flush_turn_tail_for_completion(&mut self) -> bool {
        let mut flushed = self.drain_tool_frontier_and_resume_assistant_stream();
        flushed |= self.flush_deferred_streaming_assistant_text();
        flushed |= self.flush_answer_stream();
        flushed |= self.flush_complete_active_cells();
        flushed |= self.flush_deferred_canonical_assistant_text();
        flushed |= self.flush_deferred_streaming_assistant_text();
        flushed |= self.flush_answer_stream();
        flushed
    }

    fn flush_provider_completion_tail(&mut self) -> bool {
        let mut flushed = self.drain_tool_frontier_and_resume_assistant_stream();
        flushed |= self.flush_deferred_streaming_assistant_text();
        flushed |= self.flush_answer_stream();
        if self.active_turn.cells.is_empty() {
            flushed |= self.flush_deferred_canonical_assistant_text();
            flushed |= self.flush_deferred_streaming_assistant_text();
            flushed |= self.flush_answer_stream();
        }
        flushed
    }

    fn start_tool_active_cell(
        &mut self,
        call_id: ToolCallId,
        tool_name: String,
        detail: Option<String>,
    ) {
        self.record_turn_work_activity();
        if is_exploration_tool_name(&tool_name) {
            let call_id_string = call_id.to_string();
            if let Some(ActiveCell::Exploration(cell)) = self
                .active_turn
                .cells
                .iter_mut()
                .find(|cell| matches!(cell, ActiveCell::Exploration(group) if group.contains_call_id(&call_id_string)))
            {
                cell.update_detail(&call_id_string, detail);
                self.active_turn.revision = self.active_turn.revision.wrapping_add(1);
                return;
            }
            self.flush_answer_stream();
            if let Some(ActiveCell::Exploration(cell)) = self.active_turn.cells.back_mut() {
                cell.append_or_update(tool_name, call_id_string, detail);
            } else {
                self.active_turn.cells.push_back(ActiveCell::Exploration(
                    ToolExplorationHistoryCell::new(tool_name, call_id_string, detail),
                ));
            }
            self.active_turn.revision = self.active_turn.revision.wrapping_add(1);
            return;
        }
        if let Some(ActiveCell::Tool(cell)) = self.active_turn.cells.iter_mut().find(
            |cell| matches!(cell, ActiveCell::Tool(tool) if tool.call_id == call_id.to_string()),
        ) {
            if cell.detail.is_none() {
                cell.detail = detail;
            }
            return;
        }
        self.flush_answer_stream();
        self.active_turn
            .cells
            .push_back(ActiveCell::Tool(ToolCallHistoryCell::new(
                "Calling",
                tool_name,
                call_id.to_string(),
                detail,
                None,
            )));
        self.active_turn.revision = self.active_turn.revision.wrapping_add(1);
    }

    fn complete_tool_active_cell(&mut self, result: ToolResultEnvelope) -> bool {
        let result_call_id = result.call_id.clone();
        if self
            .active_turn
            .committed_tool_result_call_ids
            .contains(&result_call_id)
        {
            return false;
        }
        self.record_turn_work_activity();
        let call_id = result.call_id.to_string();
        let mut updated = false;
        if let Some(ActiveCell::Tool(cell)) = self
            .active_turn
            .cells
            .iter_mut()
            .find(|cell| matches!(cell, ActiveCell::Tool(tool) if tool.call_id == call_id))
        {
            cell.verb = "Called";
            cell.result = Some(result.clone());
            updated = true;
        }
        if !updated
            && is_exploration_tool_name(&result.tool_name)
            && let Some(ActiveCell::Exploration(cell)) =
                self.active_turn.cells.iter_mut().find(|cell| {
                    matches!(cell, ActiveCell::Exploration(group) if group.contains_call_id(&call_id))
                })
        {
            updated = cell.complete(result.clone());
        }

        if !updated {
            self.active_turn
                .cells
                .push_back(ActiveCell::Tool(ToolCallHistoryCell::new(
                    "Called",
                    result.tool_name.clone(),
                    call_id,
                    None,
                    Some(result),
                )));
            updated = true;
        }

        self.active_turn
            .committed_tool_result_call_ids
            .insert(result_call_id);
        self.active_turn.revision = self.active_turn.revision.wrapping_add(1);
        let flushed = self.drain_tool_frontier_and_resume_assistant_stream();
        let deferred = self.flush_deferred_canonical_assistant_text();
        updated || flushed || deferred
    }

    fn handle_ask_signal(
        &mut self,
        call_id: &ToolCallId,
        arguments: Option<&serde_json::Value>,
    ) -> bool {
        self.request_queue_refresh_now();
        let _prompt = arguments
            .and_then(|arguments| arguments.get("question"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Agent is waiting for input.");
        self.set_task_status(
            "Waiting on input",
            Some(format!("ask {}", short_id_string(&call_id.to_string()))),
        );
        true
    }

    fn reconcile_canonical_tool_call(&mut self, call: &bt_core::ToolCall) -> bool {
        if is_ask_tool(&call.tool_name) {
            let _ = self.handle_ask_signal(
                &ToolCallId::new(call.call_id.clone()),
                Some(&call.arguments),
            );
            // Keep the bottom-surface question active, but let the canonical assistant
            // message print into durable history so the prompt survives reconnects.
            return false;
        }
        let call_id = ToolCallId::new(call.call_id.clone());
        if self.printed_tool_call_ids.contains(&call_id.to_string()) {
            return true;
        }
        if self
            .active_turn
            .committed_tool_result_call_ids
            .contains(&call_id)
        {
            return true;
        }
        self.start_streaming_tool_preview(
            call_id.clone(),
            call.tool_name.clone(),
            Some(&call.arguments),
        );
        self.start_tool_active_cell(
            call_id,
            call.tool_name.clone(),
            summarize_tool_detail(&call.tool_name, &call.arguments),
        );
        true
    }

    fn reconcile_canonical_tool_result(&mut self, result: &ToolResultEnvelope) -> bool {
        if is_ask_tool(&result.tool_name) {
            self.request_queue_refresh_now();
            self.clear_pending_tool(&result.call_id);
            return false;
        }
        if self
            .printed_tool_call_ids
            .contains(&result.call_id.to_string())
        {
            return true;
        }
        if self
            .active_turn
            .committed_tool_result_call_ids
            .contains(&result.call_id)
        {
            return true;
        }
        self.clear_pending_tool(&result.call_id);
        self.active_turn
            .streaming_tool_arguments
            .remove(&result.call_id);
        self.complete_tool_active_cell(result.clone())
    }

    fn active_or_committed_tool_call(&self, call_id: &ToolCallId) -> bool {
        self.active_turn
            .committed_tool_result_call_ids
            .contains(call_id)
            || self.printed_tool_call_ids.contains(&call_id.to_string())
            || self
                .active_turn
                .cells
                .iter()
                .any(|cell| cell.contains_call_id(call_id))
    }

    fn reconcile_loaded_tool_messages_with_live_cells(&mut self, min_seq_exclusive: Option<i64>) {
        let loaded_tool_messages = self
            .messages
            .iter()
            .zip(self.message_seq_ids.iter().copied())
            .filter_map(|(message, seq_id)| {
                if let Some(min_seq_exclusive) = min_seq_exclusive
                    && seq_id.is_some_and(|seq_id| seq_id <= min_seq_exclusive)
                {
                    return None;
                }
                if message.tool_call().is_none() && message.tool_result().is_none() {
                    return None;
                }
                Some((
                    crate::transcript_view::message_print_key(message),
                    message.clone(),
                ))
            })
            .collect::<Vec<_>>();

        for (message_key, message) in loaded_tool_messages {
            let mut handled_by_live_cell = false;
            if let Some(call) = message.tool_call() {
                let call_id = ToolCallId::new(call.call_id.clone());
                if self.active_or_committed_tool_call(&call_id) {
                    handled_by_live_cell |= self.reconcile_canonical_tool_call(call);
                }
            }
            if let Some(result) = message.tool_result() {
                if self.active_or_committed_tool_call(&result.call_id) {
                    handled_by_live_cell |= self.reconcile_canonical_tool_result(result);
                }
            }
            if handled_by_live_cell {
                self.printed_message_ids.insert(message_key);
            }
        }
    }

    fn current_assistant_prefix_text(&self) -> String {
        let mut prefix = self.active_turn.committed_assistant_text.clone();
        if let Some(controller) = self.active_turn.stream_controller.as_ref() {
            prefix.push_str(controller.raw_text());
        }
        prefix
    }

    fn queue_or_replace_deferred_canonical_assistant_text(&mut self, text: String) {
        match self.active_turn.deferred_canonical_assistant_text.as_mut() {
            Some(existing) if existing.len() >= text.len() => {}
            Some(existing) => *existing = text,
            None => self.active_turn.deferred_canonical_assistant_text = Some(text),
        }
    }

    fn queue_canonical_assistant_suffix(&mut self, suffix: &str, continuation: bool) -> bool {
        if suffix.is_empty() {
            return false;
        }
        self.active_turn.committed_assistant_text.push_str(suffix);
        self.queue_assistant_markdown_history_text(
            suffix.to_owned(),
            Some(if continuation {
                ScrollbackSeparator::None
            } else {
                transcript_separator_between(
                    self.last_history_kind(),
                    TranscriptEntryKind::Assistant,
                )
            }),
        )
    }

    fn flush_deferred_canonical_assistant_text(&mut self) -> bool {
        let Some(text) = self.active_turn.deferred_canonical_assistant_text.take() else {
            return false;
        };
        self.reconcile_canonical_assistant_text(&text)
    }

    fn canonical_assistant_visible_text(message: &Message) -> String {
        message
            .parts
            .iter()
            .filter_map(|part| match part {
                MessagePart::Text { text } => Some(text.as_str()),
                MessagePart::Refusal {
                    text: Some(text), ..
                } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    fn has_loaded_assistant_text_after(&self, min_seq_exclusive: Option<i64>) -> bool {
        self.messages
            .iter()
            .zip(self.message_seq_ids.iter().copied())
            .any(|(message, seq_id)| {
                if let Some(min_seq_exclusive) = min_seq_exclusive
                    && seq_id.is_some_and(|seq_id| seq_id <= min_seq_exclusive)
                {
                    return false;
                }
                message.role == Role::Assistant
                    && message.tool_call().is_none()
                    && message.tool_result().is_none()
                    && !Self::canonical_assistant_visible_text(message).is_empty()
            })
    }

    fn flush_complete_tool_cells_before_loaded_assistant(
        &mut self,
        min_seq_exclusive: Option<i64>,
    ) -> bool {
        if !self.has_loaded_assistant_text_after(min_seq_exclusive) {
            return false;
        }

        let mut flushed = false;
        while matches!(self.active_turn.cells.front(), Some(cell) if cell.is_complete()) {
            flushed |= self.flush_active_cell(TranscriptEntryKind::ToolCall);
        }
        if flushed {
            flushed |= self.emit_final_work_separator_if_needed();
        }
        flushed
    }

    fn reconcile_canonical_assistant_text(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return true;
        }
        if !self.active_turn.cells.is_empty() {
            self.queue_or_replace_deferred_canonical_assistant_text(text.to_owned());
            let _ = self.drain_tool_frontier_and_resume_assistant_stream();
            return true;
        }

        let prefix = self.current_assistant_prefix_text();
        if !prefix.is_empty() {
            let trimmed_prefix = prefix.trim_end_matches('\n');
            if text == prefix || text == trimmed_prefix {
                return true;
            }
            if let Some(suffix) = text.strip_prefix(&prefix) {
                return self.queue_canonical_assistant_suffix(suffix, true);
            }
            if let Some(suffix) = text.strip_prefix(trimmed_prefix) {
                return self.queue_canonical_assistant_suffix(suffix, true);
            }
            return self.queue_canonical_assistant_suffix(text, false);
        }

        self.queue_canonical_assistant_suffix(text, false)
    }

    fn reconcile_canonical_assistant_message_new(&mut self, message: &Message) -> bool {
        let text = Self::canonical_assistant_visible_text(message);
        if text.is_empty() {
            return false;
        }
        self.reconcile_canonical_assistant_text(&text)
    }

    pub(super) fn rebuild_committed_history_from_transcript(&mut self) {
        self.committed_history_entries.clear();
        self.pending_history_entries.clear();
        self.printed_message_ids.clear();
        self.printed_tool_call_ids.clear();
        self.printed_operator_command_keys.clear();
        self.reconcile_loaded_tool_messages_with_live_cells(None);
        let _ = self.flush_complete_tool_cells_before_loaded_assistant(None);
        let _ = self.queue_unprinted_scrollback_entries(None);
    }

    pub(super) fn rebuild_committed_history_for_scrollback_reset(&mut self) {
        self.rebuild_committed_history_from_transcript();
        self.pending_history_entries.clear();
    }

    pub(super) async fn bootstrap_event_cursor(&mut self) -> Result<(), Box<dyn Error>> {
        let response = self.client.inspect_session(self.session_id).await?;
        self.last_event_id = response.inspection.last_seq_id;
        Ok(())
    }

    pub(super) async fn ensure_event_stream(&mut self) -> Result<(), Box<dyn Error>> {
        if self
            .stream_task
            .as_ref()
            .is_some_and(|task| !task.is_finished())
            && self.stream_updates.is_some()
        {
            return Ok(());
        }
        if let Some(retry_at) = self.stream_retry_at
            && Instant::now() < retry_at
        {
            return Ok(());
        }
        if self.last_event_id.is_none() {
            self.bootstrap_event_cursor().await?;
        }

        self.stop_event_stream();
        let (sender, receiver) = mpsc::unbounded_channel();
        let client = self.client.clone();
        let session_id = self.session_id;
        let last_event_id = self.last_event_id;
        self.stream_updates = Some(receiver);
        self.stream_task = Some(tokio::spawn(async move {
            match client.stream_events(session_id, last_event_id).await {
                Ok(mut stream) => {
                    while let Some(item) = stream.next().await {
                        match item {
                            Ok(raw) => {
                                if raw.event == "error" {
                                    let message = serde_json::from_value::<ErrorEnvelope>(raw.data)
                                        .map(|error| {
                                            format!(
                                                "stream error: {} ({})",
                                                error.message, error.code
                                            )
                                        })
                                        .unwrap_or_else(|error| {
                                            format!("failed to parse stream error: {error}")
                                        });
                                    let _ = sender.send(ChatStreamUpdate::Error(message));
                                    return;
                                }

                                match serde_json::from_value::<EventEnvelope>(raw.data) {
                                    Ok(event) => {
                                        let _ =
                                            sender.send(ChatStreamUpdate::Event(Box::new(event)));
                                    }
                                    Err(error) => {
                                        let _ = sender.send(ChatStreamUpdate::Error(format!(
                                            "failed to parse streamed event: {error}"
                                        )));
                                        return;
                                    }
                                }
                            }
                            Err(error) => {
                                let _ = sender.send(ChatStreamUpdate::Error(format!(
                                    "event stream failed: {error}"
                                )));
                                return;
                            }
                        }
                    }
                    let _ = sender.send(ChatStreamUpdate::Closed);
                }
                Err(error) => {
                    let _ = sender.send(ChatStreamUpdate::Error(format!(
                        "failed to open event stream: {error}"
                    )));
                }
            }
        }));
        self.stream_retry_at = None;
        self.stream_retry_attempt = 0;
        Ok(())
    }

    pub(super) fn stop_event_stream(&mut self) {
        if let Some(task) = self.stream_task.take() {
            task.abort();
        }
        self.stream_updates = None;
    }

    pub(super) fn reset_event_stream(&mut self) {
        self.stop_event_stream();
        self.last_event_id = None;
        self.stream_retry_at = None;
        self.stream_retry_attempt = 0;
        self.active_turn.live = false;
        self.clear_live_turn_items();
        self.active_turn.optimistic_user_text = None;
        self.pending_visible_clear = false;
        self.pending_scrollback_reset_text = None;
        self.pending_scrollback_reset_lines = None;
        self.pending_history_entries.clear();
        self.active_turn.committed_assistant_text.clear();
        self.active_turn.deferred_streaming_assistant_text.clear();
        self.active_turn.deferred_canonical_assistant_text = None;
        self.active_turn.committed_tool_result_call_ids.clear();
    }

    pub(super) fn schedule_stream_retry(&mut self) {
        let delay = next_stream_retry_delay(self.stream_retry_attempt);
        self.stream_retry_at = Some(Instant::now() + delay);
        self.stream_retry_attempt = self.stream_retry_attempt.saturating_add(1);
    }

    pub(super) fn apply_stream_update(&mut self, update: ChatStreamUpdate) {
        match update {
            ChatStreamUpdate::Event(event) => self.apply_stream_event(*event),
            ChatStreamUpdate::Error(message) => {
                self.show_error(message);
                self.stop_event_stream();
                self.schedule_stream_retry();
            }
            ChatStreamUpdate::Closed => {
                self.stop_event_stream();
                self.schedule_stream_retry();
            }
        }
    }

    pub(super) fn apply_stream_event(&mut self, event: EventEnvelope) {
        let event_seq_id = event.seq_id;
        if let Some(event_seq_id) = event_seq_id
            && self
                .last_event_id
                .is_some_and(|last_event_id| event_seq_id <= last_event_id)
        {
            return;
        }
        self.last_event_id = event_seq_id.or(self.last_event_id);
        match event.payload {
            EventPayload::SessionSettingsUpdated {
                connection_id,
                model_id,
                tool_mode,
                ..
            } => {
                self.connection_id = ConnectionId::new(connection_id);
                self.tool_mode = parse_tool_mode(&tool_mode).unwrap_or(SessionToolMode::Extended);
                self.connection_model = model_id.or_else(|| {
                    self.connections
                        .iter()
                        .find(|connection| connection.id == self.connection_id)
                        .map(|connection| connection.default_model.clone())
                });
                self.last_metadata_refresh = Instant::now() - METADATA_REFRESH_INTERVAL;
            }
            EventPayload::BranchActivated { branch_id } => {
                self.branch_id = branch_id;
                self.last_metadata_refresh = Instant::now() - METADATA_REFRESH_INTERVAL;
            }
            EventPayload::BranchCreated { .. } | EventPayload::BranchSummarized { .. } => {
                self.last_metadata_refresh = Instant::now() - METADATA_REFRESH_INTERVAL;
            }
            EventPayload::MessageAppended { message } => {
                if message.role == Role::User
                    || message.tool_call().is_some()
                    || message.tool_result().is_some()
                {
                    self.mark_live_turn_start();
                    self.active_turn.live = true;
                }
                let message_key = crate::transcript_view::message_print_key(&message);
                if self
                    .messages
                    .iter()
                    .any(|existing| existing.message_id == message.message_id)
                {
                    self.refresh_tool_views();
                    self.on_messages_changed();
                    self.stream_retry_attempt = 0;
                    return;
                }
                let matched_optimistic_user = self
                    .active_turn
                    .optimistic_user_text
                    .as_deref()
                    .is_some_and(|expected_text| {
                        message_has_text(&message, Role::User, expected_text)
                    });
                if matched_optimistic_user {
                    self.messages.push(message);
                    self.message_seq_ids.push(event_seq_id);
                    if let Some(message) = self.messages.last().cloned() {
                        self.queue_scrollback_message(&message, event_seq_id);
                    }
                    self.clear_optimistic_user_state();
                } else {
                    let mut handled_by_active_turn = false;
                    if let Some(call) = message.tool_call() {
                        handled_by_active_turn |= self.reconcile_canonical_tool_call(call);
                    }
                    if let Some(result) = message.tool_result() {
                        handled_by_active_turn |= self.reconcile_canonical_tool_result(result);
                    }
                    if message.role == Role::Assistant {
                        handled_by_active_turn |=
                            self.reconcile_canonical_assistant_message_new(&message);
                    }
                    self.messages.push(message);
                    self.message_seq_ids.push(event_seq_id);
                    if handled_by_active_turn {
                        self.printed_message_ids.insert(message_key);
                    } else if let Some(message) = self.messages.last().cloned() {
                        self.queue_scrollback_message(&message, event_seq_id);
                    }
                }
                self.refresh_tool_views();
                self.on_messages_changed();
                self.stream_retry_attempt = 0;
            }
            EventPayload::CompletionRequested {
                provider, model, ..
            } => {
                self.mark_live_turn_start();
                self.active_turn.live = true;
                self.start_stream_controller_if_needed();
                self.set_task_status("Working", Some(format!("{provider} ({model})")));
            }
            EventPayload::CompletionChunk { deltas, .. } => {
                let mut live_output_changed = false;

                for delta in &deltas {
                    if let Some(delta_text) = visible_streaming_delta_text(
                        delta,
                        self.show_reasoning,
                        self.transcript_density(),
                    ) && !delta_text.is_empty()
                    {
                        live_output_changed |=
                            self.drain_tool_frontier_and_resume_assistant_stream();
                        if !self.active_turn.cells.is_empty() {
                            self.active_turn
                                .deferred_streaming_assistant_text
                                .push_str(&delta_text);
                            live_output_changed |=
                                self.drain_tool_frontier_and_resume_assistant_stream();
                        } else {
                            live_output_changed |= self.push_streaming_assistant_text_now(
                                &delta_text,
                                self.show_reasoning,
                                self.transcript_density(),
                            );
                        }
                    }

                    match delta {
                        CompletionDelta::OpenToolCall {
                            call_id,
                            tool_name,
                            arguments,
                        } => {
                            if is_ask_tool(tool_name) {
                                live_output_changed |= self.handle_ask_signal(
                                    &ToolCallId::new(call_id.clone()),
                                    arguments.as_ref(),
                                );
                                continue;
                            }
                            self.start_streaming_tool_preview(
                                ToolCallId::new(call_id.clone()),
                                tool_name.clone(),
                                arguments.as_ref(),
                            );
                            self.start_tool_active_cell(
                                ToolCallId::new(call_id.clone()),
                                tool_name.clone(),
                                arguments.as_ref().and_then(|arguments| {
                                    summarize_tool_detail(tool_name, arguments)
                                }),
                            );
                            live_output_changed = true;
                        }
                        CompletionDelta::AppendToolCallArguments {
                            call_id,
                            partial_json,
                        } => {
                            self.append_streaming_tool_preview_arguments(
                                &ToolCallId::new(call_id.clone()),
                                partial_json,
                            );
                            let updated_detail =
                                self.pending_tool_detail(&ToolCallId::new(call_id.clone()));
                            if let Some(ActiveCell::Tool(cell)) = self
                                .active_turn
                                .cells
                                .iter_mut()
                                .find(|cell| matches!(cell, ActiveCell::Tool(tool) if tool.call_id == *call_id))
                            {
                                cell.detail = updated_detail;
                                self.active_turn.revision =
                                    self.active_turn.revision.wrapping_add(1);
                            } else if let Some(ActiveCell::Exploration(cell)) = self
                                .active_turn
                                .cells
                                .iter_mut()
                                .find(|cell| {
                                    matches!(
                                        cell,
                                        ActiveCell::Exploration(group)
                                            if group.contains_call_id(call_id)
                                    )
                                })
                            {
                                cell.update_detail(call_id, updated_detail);
                                self.active_turn.revision =
                                    self.active_turn.revision.wrapping_add(1);
                            }
                            live_output_changed = true;
                        }
                        CompletionDelta::CloseToolCall { .. }
                        | CompletionDelta::AppendText { .. }
                        | CompletionDelta::AppendReasoning { .. }
                        | CompletionDelta::AppendRefusal { .. }
                        | CompletionDelta::SetStructuredOutput { .. } => {}
                    }
                }

                if live_output_changed {
                    if self.should_follow_messages() {
                        self.scroll_to_bottom();
                    }
                }
            }
            EventPayload::CompletionFinished {
                provider, model, ..
            } => {
                let _ = self.flush_provider_completion_tail();
                if self.active_turn.cells.is_empty() {
                    let _ = self.emit_final_work_separator_if_needed();
                }
                self.status = format!(
                    "Ready. Completed {} ({}). {} messages loaded.",
                    provider,
                    model,
                    self.messages.len()
                );
            }
            EventPayload::SessionError {
                class,
                code,
                message,
                ..
            } => {
                self.active_turn.live = false;
                let _ = self.flush_turn_tail_for_completion();
                let _ = self.emit_final_work_separator_if_needed();
                self.clear_live_turn_boundary();
                self.clear_live_turn_items();
                self.clear_task_status();
                self.request_queue_refresh();
                self.status = format!("Error: {code}");
                self.show_error(format!("{class} error: {message}"));
            }
            EventPayload::TurnFinished { status, .. } => {
                self.active_turn.live = false;
                let _ = self.flush_turn_tail_for_completion();
                let _ = self.emit_final_work_separator_if_needed();
                self.clear_live_turn_boundary();
                self.clear_live_turn_items();
                self.clear_task_status();
                if status == "failed" {
                    self.status = "Turn failed".to_owned();
                }
            }
            EventPayload::ToolCallRequested {
                call_id,
                tool_name,
                arguments,
            } => {
                self.mark_live_turn_start();
                self.active_turn.live = true;
                if is_ask_tool(&tool_name) {
                    self.handle_ask_signal(&call_id, Some(&arguments));
                    return;
                }
                self.start_streaming_tool_preview(
                    call_id.clone(),
                    tool_name.clone(),
                    Some(&arguments),
                );
                self.start_tool_active_cell(
                    call_id.clone(),
                    tool_name.clone(),
                    summarize_tool_detail(&tool_name, &arguments),
                );
            }
            EventPayload::ToolApprovalRequested {
                tool_name, call_id, ..
            } => {
                self.request_queue_refresh();
                self.upsert_pending_tool(
                    call_id.clone(),
                    tool_name.clone(),
                    self.pending_tool_detail(&call_id),
                );
                self.set_task_status(
                    "Waiting on approval",
                    Some(format!(
                        "{} {}",
                        tool_name,
                        short_id_string(&call_id.to_string())
                    )),
                );
            }
            EventPayload::ToolApprovalResolved {
                call_id,
                tool_name,
                decision,
                ..
            } => {
                self.request_queue_refresh();
                self.clear_pending_tool(&call_id);
                let notice = match decision {
                    ApprovalDecision::Approved { .. } => {
                        self.set_task_status(
                            "Working",
                            Some(format!(
                                "{} {}",
                                tool_name,
                                short_id_string(&call_id.to_string())
                            )),
                        );
                        "Approved the latest pending tool call.".to_owned()
                    }
                    ApprovalDecision::Denied { .. } => {
                        self.clear_task_status();
                        "Denied the latest pending tool call.".to_owned()
                    }
                };
                self.show_notice(notice);
            }
            EventPayload::ToolExecutionFinished {
                call_id,
                tool_name,
                result,
            } => {
                if is_ask_tool(&tool_name) {
                    self.request_queue_refresh_now();
                    self.clear_pending_tool(&call_id);
                    self.clear_task_status();
                    self.show_notice("Input recorded.".to_owned());
                    return;
                }
                self.request_queue_refresh();
                self.clear_pending_tool(&call_id);
                self.complete_tool_active_cell(result);
                self.set_task_status("Working", None);
            }
            EventPayload::SessionCancelled { reason } => {
                self.request_queue_refresh();
                self.clear_local_pending_turn_after_cancel();
                self.show_notice(format!("Session cancelled: {reason}"));
            }
            EventPayload::SessionCancelCleared { reason } => {
                self.request_queue_refresh();
                self.show_notice(format!("Cancel request cleared: {reason}"));
            }
            EventPayload::SessionSteered { message, .. } => {
                self.request_queue_refresh();
                self.show_notice(format!("Steered session: {message}"));
            }
            EventPayload::SessionQueuedMessageEnqueued { .. }
            | EventPayload::SessionQueuedMessageResolved { .. }
            | EventPayload::SessionSteersResolved { .. } => {
                self.request_queue_refresh();
            }
            EventPayload::OperatorCommandRecorded {
                command_type,
                raw_input,
                output,
                success,
            } => {
                self.operator_commands.push(RecordedOperatorCommand {
                    seq_id: event_seq_id,
                    occurred_at: event.occurred_at,
                    command_type,
                    raw_input,
                    output,
                    success,
                });
                if let Some(command) = self.operator_commands.last().cloned() {
                    self.queue_scrollback_operator_command(&command);
                }
                if self.should_follow_messages() {
                    self.scroll_to_bottom();
                }
            }
            _ => {}
        }
        self.last_refresh = Instant::now();
    }

    pub(super) fn merge_latest_transcript_page(&mut self, page: LoadedTranscriptPage) {
        let previous_latest_seq = self.latest_loaded_seq();
        let reconciled_optimistic_user = merge_messages(
            &mut self.messages,
            &mut self.message_seq_ids,
            page.messages,
            page.message_seq_ids,
            self.active_turn.optimistic_user_text.as_deref(),
        );
        if reconciled_optimistic_user {
            self.clear_optimistic_user_state();
        }
        merge_operator_commands(&mut self.operator_commands, page.operator_commands);
        if previous_latest_seq.is_some() {
            self.reconcile_loaded_tool_messages_with_live_cells(previous_latest_seq);
            let _ = self.flush_complete_tool_cells_before_loaded_assistant(previous_latest_seq);
        }
        self.has_more_history_before |= page.has_more_before;
        self.last_event_id = self.last_event_id.max(page.last_event_id);
        if let Some(previous_latest_seq) = previous_latest_seq {
            self.queue_unprinted_scrollback_entries(Some(previous_latest_seq));
        } else {
            self.rebuild_committed_history_from_transcript();
        }
    }

    pub(super) fn prepend_older_transcript_page(&mut self, page: LoadedTranscriptPage) {
        let reconciled_optimistic_user = merge_messages(
            &mut self.messages,
            &mut self.message_seq_ids,
            page.messages,
            page.message_seq_ids,
            self.active_turn.optimistic_user_text.as_deref(),
        );
        if reconciled_optimistic_user {
            self.clear_optimistic_user_state();
        }
        merge_operator_commands(&mut self.operator_commands, page.operator_commands);
        self.has_more_history_before = page.has_more_before;
        self.last_event_id = self.last_event_id.max(page.last_event_id);
    }

    pub(super) fn clear_optimistic_user_state(&mut self) {
        self.active_turn.optimistic_user_text = None;
    }
}
