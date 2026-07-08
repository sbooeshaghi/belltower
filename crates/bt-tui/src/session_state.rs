use super::*;

impl ChatApp {
    pub(super) async fn refresh(&mut self) -> Result<(), Box<dyn Error>> {
        let transcript = if self.messages.is_empty() && self.operator_commands.is_empty() {
            load_full_transcript(self.client.clone(), self.session_id, self.branch_id).await?
        } else {
            load_transcript_page(self.client.clone(), self.session_id, self.branch_id, None).await?
        };
        self.merge_latest_transcript_page(transcript);
        self.refresh_tool_views();
        self.refresh_queue_cache(true).await?;
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
        Ok(())
    }

    pub(super) async fn refresh_metadata(&mut self) -> Result<(), Box<dyn Error>> {
        let listed_sessions = self.client.list_sessions().await?.sessions;
        let current_session = listed_sessions
            .iter()
            .find(|session| session.session_id == self.session_id)
            .cloned();
        if let Some(session) = current_session.as_ref() {
            self.connection_id = session.connection_id.clone();
            self.tool_mode = session.tool_mode;
            self.project_root = session.project_root.to_string();
        }
        self.sessions = sorted_sessions(&listed_sessions)
            .into_iter()
            .cloned()
            .collect();
        self.connections = self.client.connections().await?.connections;
        self.connection_model = current_session
            .as_ref()
            .and_then(|session| session.model_id.clone())
            .or_else(|| {
                self.connections
                    .iter()
                    .find(|connection| connection.id == self.connection_id)
                    .map(|connection| connection.default_model.clone())
            });
        self.branches = self.client.branches(self.session_id).await?.branches;
        self.last_metadata_refresh = Instant::now();
        Ok(())
    }

    pub(super) fn apply_session_metadata(&mut self, session: SessionRecord) {
        self.connection_id = session.connection_id.clone();
        self.tool_mode = session.tool_mode;
        self.project_root = session.project_root.to_string();
        self.connection_model = session.model_id.clone().or_else(|| {
            self.connections
                .iter()
                .find(|connection| connection.id == session.connection_id)
                .map(|connection| connection.default_model.clone())
        });

        match self
            .sessions
            .iter_mut()
            .find(|candidate| candidate.session_id == session.session_id)
        {
            Some(existing) => *existing = session,
            None => self.sessions.push(session),
        }
        self.sessions = sorted_sessions(&self.sessions)
            .into_iter()
            .cloned()
            .collect();
    }

    pub(super) async fn refresh_queue_cache(&mut self, force: bool) -> Result<(), Box<dyn Error>> {
        if !force
            && (!self.queue_refresh_requested
                || self.last_queue_refresh.elapsed() < QUEUE_REFRESH_INTERVAL)
        {
            return Ok(());
        }

        let response = self.client.session_queue(self.session_id).await?;
        self.apply_queue_inspection(response.inspection);
        self.last_queue_refresh = Instant::now();
        self.queue_refresh_requested = false;
        Ok(())
    }

    pub(super) fn request_queue_refresh(&mut self) {
        self.queue_refresh_requested = true;
    }

    pub(super) fn request_queue_refresh_now(&mut self) {
        self.queue_refresh_requested = true;
        self.last_queue_refresh = Instant::now() - QUEUE_REFRESH_INTERVAL;
    }

    pub(super) fn apply_queue_inspection(&mut self, inspection: SessionQueueInspection) {
        self.pending_approvals = inspection.pending_approvals.clone();
        self.pending_inputs = inspection.pending_inputs.clone();
        self.queue_inspection = Some(inspection);
        self.clamp_approval_menu_selection();
        self.clamp_question_choice_selection();
    }

    pub(super) async fn refresh_readiness_cache(
        &mut self,
        force: bool,
    ) -> Result<(), Box<dyn Error>> {
        if !force && self.last_readiness_refresh.elapsed() < READINESS_REFRESH_INTERVAL {
            return Ok(());
        }

        let (status, backends, models) = tokio::try_join!(
            self.client.status_inspection(),
            self.client.model_backends(),
            self.client.connection_models()
        )?;
        self.connection_status = Some(status.inspection);
        self.model_backends = backends.backends;
        self.connection_models = models.connections;
        self.last_readiness_refresh = Instant::now();
        Ok(())
    }

    pub(super) async fn refresh_connection_model_inventory(
        &mut self,
        connection_id: &ConnectionId,
    ) -> Result<(), Box<dyn Error>> {
        let models = self
            .client
            .connection_model_inventory(connection_id)
            .await?;
        self.merge_connection_model_inventories(models.connections);
        self.last_readiness_refresh = Instant::now();
        Ok(())
    }

    pub(super) fn merge_connection_model_inventories(
        &mut self,
        inventories: Vec<ConnectionModelInventory>,
    ) {
        for inventory in inventories {
            if let Some(existing) = self
                .connection_models
                .iter_mut()
                .find(|existing| existing.connection_id == inventory.connection_id)
            {
                *existing = inventory;
            } else {
                self.connection_models.push(inventory);
            }
        }
    }

    pub(super) async fn refresh_mcp_cache(&mut self, force: bool) -> Result<(), Box<dyn Error>> {
        if !force && self.last_mcp_refresh.elapsed() < MCP_REFRESH_INTERVAL {
            return Ok(());
        }

        let inventory = self.client.mcp_inventory().await?;
        self.mcp_servers = inventory.servers;
        self.mcp_tools = inventory.tools;
        self.last_mcp_refresh = Instant::now();
        Ok(())
    }

    pub(super) fn refresh_tool_views(&mut self) {
        self.pending_tools = pending_tool_calls(&self.messages);
        self.pending_tools
            .retain(|pending| !self.resolving_tools.contains(&pending.call_id));
    }

    pub(super) fn clear_live_turn_items(&mut self) {
        self.active_turn.cells.clear();
        self.active_turn.stream_controller = None;
        self.active_turn.streaming_tool_arguments.clear();
        self.active_turn.deferred_streaming_assistant_text.clear();
        self.adaptive_chunking.reset();
    }

    pub(super) fn start_streaming_tool_preview(
        &mut self,
        call_id: ToolCallId,
        tool_name: String,
        arguments: Option<&serde_json::Value>,
    ) {
        if let Some(arguments) = arguments {
            self.active_turn
                .streaming_tool_arguments
                .insert(call_id.clone(), arguments.to_string());
        }

        let detail = arguments.and_then(|arguments| summarize_tool_detail(&tool_name, arguments));
        self.upsert_pending_tool(call_id, tool_name, detail);
    }

    pub(super) fn append_streaming_tool_preview_arguments(
        &mut self,
        call_id: &ToolCallId,
        partial_json: &str,
    ) {
        let entry = self
            .active_turn
            .streaming_tool_arguments
            .entry(call_id.clone())
            .or_default();
        entry.push_str(partial_json);
    }

    pub(super) fn upsert_pending_tool(
        &mut self,
        call_id: ToolCallId,
        tool_name: String,
        detail: Option<String>,
    ) {
        if let Some(existing) = self
            .pending_tools
            .iter_mut()
            .find(|pending| pending.call_id == call_id)
        {
            existing.tool_name = tool_name;
            if detail.is_some() {
                existing.detail = detail;
            }
            return;
        }

        self.pending_tools.push(PendingToolView {
            call_id,
            tool_name,
            detail,
        });
    }

    pub(super) fn clear_pending_tool(&mut self, call_id: &ToolCallId) {
        self.pending_tools
            .retain(|pending| &pending.call_id != call_id);
        self.active_turn.streaming_tool_arguments.remove(call_id);
        self.resolving_tools.remove(call_id);
    }

    pub(super) fn pending_tool_detail(&self, call_id: &ToolCallId) -> Option<String> {
        self.pending_tools
            .iter()
            .find(|pending| &pending.call_id == call_id)
            .and_then(|pending| pending.detail.clone())
            .or_else(|| {
                self.active_turn
                    .streaming_tool_arguments
                    .get(call_id)
                    .and_then(|arguments| serde_json::from_str::<serde_json::Value>(arguments).ok())
                    .and_then(|arguments| {
                        self.pending_tools
                            .iter()
                            .find(|pending| &pending.call_id == call_id)
                            .and_then(|pending| {
                                summarize_tool_detail(&pending.tool_name, &arguments)
                            })
                    })
            })
    }
}
