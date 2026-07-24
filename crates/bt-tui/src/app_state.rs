use super::*;
use crate::history_cell::{
    AskHistoryCell, AssistantMarkdownHistoryCell, FinalMessageSeparatorCell, UserHistoryCell,
};

impl ChatApp {
    pub(super) fn push_history_entry(&mut self, entry: HistoryEntry) -> bool {
        self.committed_history_entries.push(entry.clone());
        self.pending_history_entries.push_back(entry);
        true
    }

    pub(super) fn new(
        client: BelltowerClient,
        session_id: SessionId,
        branch_id: BranchId,
        project_root: String,
        connection_id: ConnectionId,
    ) -> Self {
        Self {
            client,
            session_id,
            branch_id,
            connection_id,
            connection_model: None,
            tool_mode: SessionToolMode::Extended,
            project_root,
            messages: Vec::new(),
            message_seq_ids: Vec::new(),
            operator_commands: Vec::new(),
            rendered_message_lines: vec![Line::from("No messages yet.")],
            wrapped_message_lines: vec![Line::from("No messages yet.")],
            visual_line_count_cache: 1,
            max_line_width_cache: 1,
            clear_separator_skip_line: None,
            sessions: Vec::new(),
            branches: Vec::new(),
            connections: Vec::new(),
            connection_status: None,
            connection_models: Vec::new(),
            model_backends: Vec::new(),
            mcp_servers: Vec::new(),
            mcp_tools: Vec::new(),
            pending_tools: Vec::new(),
            pending_approvals: Vec::new(),
            pending_inputs: Vec::new(),
            queue_inspection: None,
            queue_refresh_requested: true,
            resolving_tools: BTreeSet::new(),
            composer: ComposerState {
                view_width: composer_wrap_width(DEFAULT_TEXT_VIEW_WIDTH),
                ..ComposerState::default()
            },
            bottom_shell: BottomShellState::default(),
            status: "Connected".to_owned(),
            task_status: None,
            transient_status: None,
            transient_status_until: None,
            pending_escape_prefix_at: None,
            message_view_height: 1,
            message_view_width: DEFAULT_TEXT_VIEW_WIDTH,
            transcript_output_width: DEFAULT_TEXT_VIEW_WIDTH,
            show_reasoning: false,
            auto_follow_messages: true,
            resume_follow_lock: false,
            post_resume_layout_sync_pending: false,
            pending_hard_clear: false,
            pending_visible_clear: false,
            pending_scrollback_reset_text: None,
            pending_scrollback_reset_lines: None,
            should_quit: false,
            detach_requested: false,
            pending_send: None,
            pending_send_started_at: None,
            pending_message_submissions: VecDeque::new(),
            pending_session_load: None,
            pending_resume_command_raw_input: None,
            pending_history_backfill: None,
            pending_shell_command: None,
            pending_approval: None,
            pending_approval_started_at: None,
            pending_approval_context: None,
            cancel_requested: false,
            stream_task: None,
            stream_updates: None,
            last_event_id: None,
            stream_retry_at: None,
            stream_retry_attempt: 0,
            active_turn: ActiveTurnState::default(),
            adaptive_chunking: AdaptiveChunkingPolicy::default(),
            assistant_commit_batch_size: None,
            pending_history_entries: VecDeque::new(),
            committed_history_entries: Vec::new(),
            printed_message_ids: BTreeSet::new(),
            printed_tool_call_ids: BTreeSet::new(),
            printed_operator_command_keys: BTreeSet::new(),
            has_more_history_before: false,
            last_refresh: Instant::now(),
            last_metadata_refresh: Instant::now() - Duration::from_secs(10),
            last_queue_refresh: Instant::now() - QUEUE_REFRESH_INTERVAL,
            last_readiness_refresh: Instant::now() - READINESS_REFRESH_INTERVAL,
            last_mcp_refresh: Instant::now() - MCP_REFRESH_INTERVAL,
        }
    }

    pub(super) fn queue_history_cell(
        &mut self,
        cell: SharedHistoryCell,
        kind: TranscriptEntryKind,
        separator: Option<ScrollbackSeparator>,
    ) -> bool {
        self.push_history_entry(HistoryEntry {
            separator: separator.unwrap_or_else(|| {
                if cell.is_stream_continuation() {
                    ScrollbackSeparator::None
                } else {
                    transcript_separator_between(self.last_history_kind(), kind)
                }
            }),
            kind: Some(kind),
            cell,
        })
    }

    pub(super) fn queue_history_meta_cell(
        &mut self,
        cell: SharedHistoryCell,
        separator: ScrollbackSeparator,
    ) -> bool {
        self.push_history_entry(HistoryEntry {
            separator,
            kind: None,
            cell,
        })
    }

    pub(super) fn last_history_kind(&self) -> Option<TranscriptEntryKind> {
        self.committed_history_entries
            .iter()
            .rev()
            .find_map(|entry| entry.kind)
    }

    pub(super) fn queue_plain_history_text(
        &mut self,
        text: impl Into<String>,
        kind: TranscriptEntryKind,
        separator: Option<ScrollbackSeparator>,
    ) -> bool {
        let text = text.into();
        if text.is_empty() {
            return false;
        }
        let lines = text
            .lines()
            .map(|line| Line::from(line.to_owned()))
            .collect::<Vec<_>>();
        if matches!(kind, TranscriptEntryKind::User) {
            let styled_lines = text.lines().map(str::to_owned).collect::<Vec<_>>();
            self.queue_history_cell(
                Arc::new(UserHistoryCell::new(styled_lines)),
                kind,
                separator,
            )
        } else {
            self.queue_history_cell(Arc::new(PlainHistoryCell::new(lines)), kind, separator)
        }
    }

    pub(super) fn queue_assistant_markdown_history_text(
        &mut self,
        source: impl Into<String>,
        separator: Option<ScrollbackSeparator>,
    ) -> bool {
        let source = source.into();
        if source.trim().is_empty() {
            return false;
        }
        self.queue_history_cell(
            Arc::new(AssistantMarkdownHistoryCell::new(
                source,
                std::path::Path::new(&self.project_root),
            )),
            TranscriptEntryKind::Assistant,
            separator,
        )
    }

    fn assistant_stream_tail_len(entries: &[HistoryEntry]) -> usize {
        entries
            .iter()
            .rev()
            .take_while(|entry| {
                entry.kind == Some(TranscriptEntryKind::Assistant)
                    && entry.cell.is_assistant_stream_cell()
            })
            .count()
    }

    fn pending_assistant_stream_tail_len(entries: &VecDeque<HistoryEntry>) -> usize {
        entries
            .iter()
            .rev()
            .take_while(|entry| {
                entry.kind == Some(TranscriptEntryKind::Assistant)
                    && entry.cell.is_assistant_stream_cell()
            })
            .count()
    }

    fn assistant_markdown_entry(
        &self,
        source: &str,
        separator: ScrollbackSeparator,
    ) -> HistoryEntry {
        HistoryEntry {
            cell: Arc::new(AssistantMarkdownHistoryCell::new(
                source.to_owned(),
                std::path::Path::new(&self.project_root),
            )),
            separator,
            kind: Some(TranscriptEntryKind::Assistant),
        }
    }

    pub(super) fn consolidate_assistant_stream_history(&mut self, source: &str) -> bool {
        if source.trim().is_empty() {
            return false;
        }

        let committed_tail_len = Self::assistant_stream_tail_len(&self.committed_history_entries);
        if committed_tail_len == 0 {
            return false;
        }

        let pending_tail_len =
            Self::pending_assistant_stream_tail_len(&self.pending_history_entries);
        let committed_start = self.committed_history_entries.len() - committed_tail_len;
        let separator = self.committed_history_entries[committed_start].separator;
        let replacement = self.assistant_markdown_entry(source, separator);
        self.committed_history_entries.truncate(committed_start);
        self.committed_history_entries.push(replacement);

        // Only replace pending terminal output if no part of this stream has
        // already been inserted into the real terminal scrollback.
        if pending_tail_len == committed_tail_len {
            let pending_start = self.pending_history_entries.len() - pending_tail_len;
            let separator = self.pending_history_entries[pending_start].separator;
            for _ in 0..pending_tail_len {
                self.pending_history_entries.pop_back();
            }
            self.pending_history_entries
                .push_back(self.assistant_markdown_entry(source, separator));
        }

        true
    }

    pub(super) fn flush_active_cell(&mut self, kind: TranscriptEntryKind) -> bool {
        let Some(active) = self.active_turn.cells.pop_front() else {
            return false;
        };
        for call_id in active.call_ids() {
            self.printed_tool_call_ids.insert(call_id);
        }
        self.active_turn.revision = self.active_turn.revision.wrapping_add(1);
        self.queue_history_cell(active.into_shared(), kind, None)
    }

    pub(super) fn active_stream_tail_lines(&self) -> Vec<Line<'static>> {
        self.active_turn
            .stream_controller
            .as_ref()
            .map(|controller| {
                controller.live_tail_lines(self.show_reasoning, self.transcript_density())
            })
            .unwrap_or_default()
    }

    pub(super) fn active_view_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if !self.active_turn.cells.is_empty() {
            for active in self
                .active_turn
                .cells
                .iter()
                .filter(|active| active.visible_in_live_view())
            {
                lines.extend(active.display_lines(width));
            }
        }
        let stream_tail = self.active_stream_tail_lines();
        if !stream_tail.is_empty() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.extend(stream_tail);
        }
        lines
    }

    pub(super) fn active_view_height(&self, width: u16) -> u16 {
        let width = width.max(1);
        let mut height = 0u16;
        if !self.active_turn.cells.is_empty() {
            for active in self
                .active_turn
                .cells
                .iter()
                .filter(|active| active.visible_in_live_view())
            {
                height = height.saturating_add(active.desired_height(width));
            }
        }
        let lines = self.active_stream_tail_lines();
        if lines.is_empty() {
            height
        } else {
            if height > 0 {
                height = height.saturating_add(1);
            }
            height.saturating_add(
                Paragraph::new(Text::from(lines))
                    .wrap(Wrap { trim: false })
                    .line_count(width)
                    .try_into()
                    .unwrap_or(0),
            )
        }
    }

    pub(super) fn compose_scrollback_reset_lines(&self) -> Vec<Line<'static>> {
        let mut lines = crate::bottom_pane::render_startup_banner(self, None)
            .lines()
            .map(|line| Line::from(line.to_owned()))
            .collect::<Vec<_>>();
        lines.extend(flatten_history_entries(
            &self.committed_history_entries,
            self.transcript_output_width.max(1),
        ));
        lines
    }

    pub(super) fn show_notice(&mut self, message: impl Into<String>) {
        self.transient_status = Some(message.into());
        self.transient_status_until = Some(Instant::now() + COMMAND_NOTICE_TTL);
    }

    pub(super) fn set_task_status(&mut self, header: impl Into<String>, detail: Option<String>) {
        self.task_status = Some(TaskStatusState {
            header: header.into(),
            detail: detail.filter(|detail| !detail.trim().is_empty()),
            show_interrupt_hint: true,
        });
    }

    pub(super) fn clear_task_status(&mut self) {
        self.task_status = None;
    }

    pub(super) fn push_local_operator_command(
        &mut self,
        command_type: impl Into<String>,
        raw_input: impl Into<String>,
        output: impl Into<String>,
        success: bool,
        seq_id: Option<i64>,
    ) {
        let command_type = command_type.into();
        let raw_input = raw_input.into();
        let output = output.into();
        let now = time::OffsetDateTime::now_utc();

        if command_type == "local_error"
            && let Some(last) = self.operator_commands.last_mut()
            && last.command_type == "local_error"
            && errors_look_equivalent(&last.output, &output)
        {
            if output.len() >= last.output.len() {
                last.output = output;
            }
            last.seq_id = match (last.seq_id, seq_id) {
                (Some(left), Some(right)) => Some(left.max(right)),
                (Some(left), None) => Some(left),
                (None, Some(right)) => Some(right),
                (None, None) => None,
            };
            last.occurred_at = now;
        } else {
            self.operator_commands.push(RecordedOperatorCommand {
                seq_id,
                occurred_at: now,
                command_type,
                raw_input,
                output,
                success,
            });
        }
        self.rebuild_render_cache();
        if self.should_follow_messages() {
            self.scroll_to_bottom();
        }
    }

    pub(super) fn queue_scrollback_message(
        &mut self,
        message: &Message,
        seq_id: Option<i64>,
    ) -> bool {
        let Some(_) = seq_id else {
            return false;
        };
        if let Some(queued) = self.queue_scrollback_ask_message(message) {
            return queued;
        }
        let key = crate::transcript_view::message_print_key(message);
        if let Some(call_id) = crate::transcript_view::message_tool_call_id(message)
            && self.printed_tool_call_ids.contains(&call_id)
        {
            self.printed_message_ids.insert(key);
            return false;
        }
        let assistant_source = assistant_markdown_source(message);
        let rendered = if assistant_source.is_none() {
            render_transcript_message_with_options(
                message,
                self.show_reasoning,
                self.transcript_density(),
                self.transcript_output_width.max(1),
            )
        } else {
            Some(String::new())
        };
        if assistant_source.is_none() && rendered.is_none() {
            return false;
        }
        if !self.printed_message_ids.insert(key) {
            return false;
        }
        if let Some(source) = assistant_source {
            return self.queue_assistant_markdown_history_text(source, None);
        }
        let Some(rendered) = rendered else {
            return false;
        };
        self.queue_plain_history_text(rendered, transcript_entry_kind_for_message(message), None)
    }

    fn ask_question_for_call_id(&self, call_id: &str) -> Option<String> {
        self.messages.iter().rev().find_map(|message| {
            message.tool_call().and_then(|call| {
                (call.tool_name == "ask" && call.call_id == call_id)
                    .then(|| ask_question_from_call(call))
                    .flatten()
            })
        })
    }

    fn queue_scrollback_ask_message(&mut self, message: &Message) -> Option<bool> {
        if !message_is_ask_exchange_only(message) {
            return None;
        }

        let key = crate::transcript_view::message_print_key(message);
        if !self.printed_message_ids.insert(key) {
            return Some(false);
        }

        if let Some(result) = message
            .tool_result()
            .filter(|result| result.tool_name == "ask")
        {
            return Some(self.queue_history_cell(
                Arc::new(AskHistoryCell::new(
                    self.ask_question_for_call_id(&result.call_id.to_string()),
                    ask_answer_from_result(result),
                )),
                TranscriptEntryKind::Assistant,
                None,
            ));
        }

        Some(false)
    }

    pub(super) fn queue_scrollback_operator_command(
        &mut self,
        command: &RecordedOperatorCommand,
    ) -> bool {
        let Some(_) = command.seq_id else {
            return false;
        };
        let key = operator_command_print_key(command);
        if !self.printed_operator_command_keys.insert(key) {
            return false;
        }
        self.queue_plain_history_text(
            render_operator_command(command, self.transcript_output_width.max(1)),
            if command.command_type == "local_error" {
                TranscriptEntryKind::LocalError
            } else {
                TranscriptEntryKind::Operator
            },
            None,
        )
    }

    pub(super) fn queue_rendered_scrollback_entry(
        &mut self,
        key: &str,
        rendered: String,
        kind: TranscriptEntryKind,
    ) -> bool {
        if key.starts_with("message:") {
            if !self.printed_message_ids.insert(key.to_owned()) {
                return false;
            }
        } else if !self.printed_operator_command_keys.insert(key.to_owned()) {
            return false;
        }

        self.queue_plain_history_text(rendered, kind, None)
    }

    fn queue_assistant_markdown_scrollback_entry(&mut self, key: &str, source: String) -> bool {
        if key.starts_with("message:") {
            if !self.printed_message_ids.insert(key.to_owned()) {
                return false;
            }
        } else if !self.printed_operator_command_keys.insert(key.to_owned()) {
            return false;
        }

        self.queue_assistant_markdown_history_text(source, None)
    }

    pub(super) fn queue_unprinted_scrollback_entries(
        &mut self,
        min_seq_exclusive: Option<i64>,
    ) -> bool {
        let mut queued = false;
        for entry in merged_rendered_transcript_entries(
            &self.messages,
            &self.message_seq_ids,
            &self.operator_commands,
            self.show_reasoning,
            self.transcript_density(),
            self.transcript_output_width.max(1),
        ) {
            if !entry.printable_in_scrollback {
                continue;
            }
            if let Some(min_seq_exclusive) = min_seq_exclusive {
                let Some(seq_id) = entry.seq_id else {
                    continue;
                };
                if seq_id <= min_seq_exclusive {
                    continue;
                }
            }
            let Some(key) = entry.key.as_deref() else {
                continue;
            };
            if let Some(call_id) = entry.tool_call_id.as_deref()
                && self.printed_tool_call_ids.contains(call_id)
            {
                self.printed_message_ids.insert(key.to_owned());
                continue;
            }
            if let Some(source) = entry.assistant_markdown_source {
                queued |= self.queue_assistant_markdown_scrollback_entry(key, source);
            } else {
                queued |= self.queue_rendered_scrollback_entry(key, entry.rendered, entry.kind);
            }
        }

        queued
    }

    pub(super) fn latest_loaded_seq(&self) -> Option<i64> {
        let latest_message = self
            .message_seq_ids
            .iter()
            .filter_map(|seq_id| seq_id.filter(|seq_id| *seq_id > 0))
            .max();
        let latest_command = self
            .operator_commands
            .iter()
            .filter_map(|command| command.seq_id.filter(|seq_id| *seq_id > 0))
            .max();
        match (latest_message, latest_command) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (Some(left), None) => Some(left),
            (None, Some(right)) => Some(right),
            (None, None) => None,
        }
    }

    pub(super) fn local_error_anchor_seq(&self) -> Option<i64> {
        self.last_event_id.or_else(|| self.latest_loaded_seq())
    }

    pub(super) fn show_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        let seq_id = self.local_error_anchor_seq();
        let commands_before = self.operator_commands.len();
        self.push_local_operator_command("local_error", "", message, false, seq_id);
        // A local error is not a server-sequenced command, so the scrollback
        // sync driven by loaded pages will never print it; queue it directly
        // or the operator sees nothing at all.
        if self.operator_commands.len() > commands_before
            && let Some(command) = self.operator_commands.last().cloned()
        {
            self.queue_local_error_now(&command);
        }
        self.show_notice("Error recorded in chat.");
    }

    fn queue_local_error_now(&mut self, command: &RecordedOperatorCommand) {
        let key = operator_command_print_key(command);
        if !self.printed_operator_command_keys.insert(key) {
            return;
        }
        let _ = self.queue_plain_history_text(
            render_operator_command(command, self.transcript_output_width.max(1)),
            TranscriptEntryKind::LocalError,
            None,
        );
    }

    pub(super) fn has_pending_request(&self) -> bool {
        self.pending_send.is_some() || self.pending_approval.is_some()
    }

    pub(super) fn clear_local_pending_turn_after_cancel(&mut self) {
        if let Some(handle) = self.pending_send.take() {
            handle.abort();
        }
        self.pending_send_started_at = None;
        self.pending_shell_command = None;
        self.active_turn.live = false;
        self.clear_live_turn_boundary();
        self.clear_live_turn_items();
        self.active_turn.cells.clear();
        self.active_turn.stream_controller = None;
        self.active_turn.streaming_tool_arguments.clear();
        self.active_turn.committed_assistant_text.clear();
        self.active_turn.deferred_streaming_assistant_text.clear();
        self.active_turn.deferred_canonical_assistant_text = None;
        self.active_turn.committed_tool_result_call_ids.clear();
        self.adaptive_chunking.reset();
        self.cancel_requested = false;
        self.request_queue_refresh();
        while let Some(pending) = self.pending_message_submissions.pop_front() {
            pending.handle.abort();
        }
        self.sync_transcript_view();
    }

    pub(super) fn runtime_busy_for_controls(&self) -> bool {
        self.queue_inspection.as_ref().is_some_and(|inspection| {
            matches!(
                inspection.runtime_state,
                SessionRuntimeState::Working
                    | SessionRuntimeState::WaitingOnInput
                    | SessionRuntimeState::WaitingOnApproval
                    | SessionRuntimeState::CancelRequested
            )
        })
    }

    pub(super) fn turn_is_busy(&self) -> bool {
        self.has_pending_request()
            || self.runtime_busy_for_controls()
            || !self.pending_approvals.is_empty()
            || !self.pending_inputs.is_empty()
    }

    pub(super) fn active_notice(&self) -> Option<&str> {
        let until = self.transient_status_until?;
        if Instant::now() > until {
            return None;
        }
        self.transient_status.as_deref()
    }

    pub(super) fn transcript_density(&self) -> TranscriptDensity {
        TranscriptDensity::Normal
    }

    pub(super) fn active_turn_is_live(&self) -> bool {
        self.active_turn.live
            || self.active_turn.optimistic_user_text.is_some()
            || self.has_pending_request()
            || self.runtime_busy_for_controls()
            || !self.pending_approvals.is_empty()
            || !self.pending_inputs.is_empty()
            || !self.active_turn.cells.is_empty()
            || !self.active_stream_tail_lines().is_empty()
            || pending_chat_placeholder(self).is_some()
    }

    pub(super) fn mark_live_turn_start(&mut self) {
        if self.active_turn.started_at.is_none() {
            self.active_turn.started_at = Some(Instant::now());
        }
        let new_turn_boundary =
            self.active_turn.message_start.is_none() && self.active_turn.command_start.is_none();
        if new_turn_boundary {
            self.active_turn.committed_assistant_text.clear();
            self.active_turn.deferred_streaming_assistant_text.clear();
            self.active_turn.deferred_canonical_assistant_text = None;
            self.active_turn.committed_tool_result_call_ids.clear();
            self.clear_turn_work_activity();
            self.active_turn.final_separator_emitted = false;
        }
        if self.active_turn.message_start.is_none() {
            self.active_turn.message_start = Some(self.messages.len());
        }
        if self.active_turn.command_start.is_none() {
            self.active_turn.command_start = Some(self.operator_commands.len());
        }
    }

    pub(super) fn record_turn_work_activity(&mut self) {
        if !self.active_turn.final_separator_emitted {
            self.active_turn.needs_final_separator = true;
        }
        self.active_turn.had_work_activity = true;
    }

    pub(super) fn clear_turn_work_activity(&mut self) {
        self.active_turn.needs_final_separator = false;
        self.active_turn.had_work_activity = false;
    }

    pub(super) fn emit_final_work_separator_if_needed(&mut self) -> bool {
        if !self.active_turn.cells.is_empty() {
            return false;
        }
        if self.active_turn.final_separator_emitted {
            self.clear_turn_work_activity();
            return false;
        }
        if !(self.active_turn.needs_final_separator && self.active_turn.had_work_activity) {
            self.clear_turn_work_activity();
            return false;
        }

        let elapsed_seconds = self
            .active_turn
            .started_at
            .map(|started_at| started_at.elapsed().as_secs());
        let queued = self.queue_history_meta_cell(
            Arc::new(FinalMessageSeparatorCell::new(elapsed_seconds)),
            ScrollbackSeparator::Line,
        );
        self.clear_turn_work_activity();
        self.active_turn.final_separator_emitted = queued;
        queued
    }

    pub(super) fn clear_live_turn_boundary(&mut self) {
        self.clear_turn_work_activity();
        self.active_turn.final_separator_emitted = false;
        self.clear_task_status();
        self.active_turn.started_at = None;
        self.active_turn.message_start = None;
        self.active_turn.command_start = None;
    }

    pub(super) fn remove_optimistic_user_message(&mut self) {
        let Some(_) = self.active_turn.optimistic_user_text.as_ref() else {
            return;
        };
        self.clear_optimistic_user_state();
        self.sync_transcript_view();
    }

    pub(super) fn on_messages_changed(&mut self) {
        self.rebuild_render_cache();
        if self.should_follow_messages() {
            self.scroll_to_bottom();
        }
    }

    pub(super) fn connection_exists(&self, id: &ConnectionId) -> bool {
        self.connections
            .iter()
            .any(|connection| &connection.id == id)
    }

    pub(super) fn effective_model(&self) -> &str {
        self.connection_model.as_deref().unwrap_or("default model")
    }

    pub(super) fn set_transcript_output_width(&mut self, width: u16) {
        self.transcript_output_width = width.max(1);
    }

    pub(super) fn set_input_view_width(&mut self, width: u16) {
        self.composer.view_width = width.saturating_sub(2).max(1);
    }

    pub(super) fn input_view_width(&self) -> u16 {
        self.composer.view_width
    }

    pub(super) fn set_bottom_panel_view_width(&mut self, width: u16) {
        self.bottom_shell.bottom_panel_view_width = width.max(1);
    }

    pub(super) fn bottom_panel_view_width(&self) -> u16 {
        self.bottom_shell.bottom_panel_view_width
    }

    pub(super) fn prepare_scrollback_reflow_for_resize(&mut self, width: u16) {
        self.set_transcript_output_width(width);
        self.set_input_view_width(composer_wrap_width(width));
        self.set_bottom_panel_view_width(width);
        self.rebuild_committed_history_for_scrollback_reset();
        self.request_scrollback_reset(true);
    }

    pub(super) fn set_message_view_size(&mut self, width: u16, height: u16) {
        let next_width = width.max(1);
        let next_height = height.max(1);
        if self.message_view_width != next_width || self.message_view_height != next_height {
            self.message_view_width = next_width;
            self.message_view_height = next_height;
            self.rebuild_render_cache();
        }
    }

    pub(super) fn base_rendered_lines(&self) -> Vec<Line<'static>> {
        rendered_message_lines_with_options(
            &self.messages,
            &self.message_seq_ids,
            &self.operator_commands,
            self.clear_separator_skip_line,
            self.show_reasoning,
            self.transcript_density(),
            self.message_view_width,
        )
    }

    pub(super) fn sync_transcript_view(&mut self) {
        self.rebuild_render_cache();
    }

    pub(super) fn should_follow_messages(&self) -> bool {
        self.resume_follow_lock || self.auto_follow_messages
    }

    pub(super) fn rebuild_render_cache(&mut self) {
        self.rendered_message_lines = self.base_rendered_lines();
        self.refresh_render_metrics();
    }

    pub(super) fn refresh_render_metrics(&mut self) {
        self.wrapped_message_lines =
            wrap_rendered_lines(&self.rendered_message_lines, self.message_view_width);
        self.visual_line_count_cache = self.wrapped_message_lines.len().max(1);
        self.max_line_width_cache = rendered_max_line_width(&self.rendered_message_lines);
    }

    pub(super) fn request_scrollback_reset(&mut self, hard_clear: bool) {
        self.pending_hard_clear = hard_clear;
        self.pending_visible_clear = !hard_clear;
        self.pending_scrollback_reset_text = None;
        self.pending_scrollback_reset_lines = Some(self.compose_scrollback_reset_lines());
    }
}

fn flatten_history_entries(entries: &[HistoryEntry], width: u16) -> Vec<Line<'static>> {
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
