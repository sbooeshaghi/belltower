// context.rs keeps one bounded runtime concern out of `runtime.rs`; cross-crate ownership remains unchanged.
use super::support::*;
use super::*;
use std::time::Instant;

impl BelltowerRuntime {
    pub fn messages(
        &self,
        session_id: SessionId,
        branch_id: Option<bt_core::BranchId>,
    ) -> Result<Vec<Message>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_messages(session_id, branch_id)
    }

    pub fn prepare_turn_context(
        &self,
        session: &SessionRecord,
        branch: &BranchRecord,
        system_prompt: Option<String>,
        tools: Vec<ToolSpec>,
        thinking: Option<ThinkingConfig>,
        turn_id: Option<TurnId>,
        settings_revision_id: u64,
    ) -> Result<PreparedTurnContext> {
        self.prepare_turn_context_with_options(
            session,
            branch,
            system_prompt,
            tools,
            thinking,
            turn_id,
            settings_revision_id,
            false,
        )
    }

    pub fn compact_branch_context(
        &self,
        session: &SessionRecord,
        branch: &BranchRecord,
        system_prompt: Option<String>,
        tools: Vec<ToolSpec>,
        thinking: Option<ThinkingConfig>,
    ) -> Result<PreparedTurnContext> {
        let settings_revision_id = self.current_settings_revision(session.session_id)?;
        self.prepare_turn_context_with_options(
            session,
            branch,
            system_prompt,
            tools,
            thinking,
            None,
            settings_revision_id,
            true,
        )
    }

    pub fn local_branch_messages(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
    ) -> Result<Vec<Message>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_local_branch_messages(session_id, branch_id)
    }

    pub fn resolve_instructions(
        &self,
        project_root: Option<&camino::Utf8Path>,
    ) -> Result<Vec<bt_core::InstructionDocument>> {
        self.instructions.resolve(project_root)
    }

    #[must_use]
    pub fn core_prompt_document(&self) -> bt_core::InstructionDocument {
        self.instructions.core_prompt()
    }

    #[must_use]
    pub fn provider_prompt_overlay(
        &self,
        provider_family: &str,
    ) -> Option<bt_core::InstructionDocument> {
        self.instructions.provider_overlay(provider_family)
    }

    pub(super) fn prepare_turn_context_with_options(
        &self,
        session: &SessionRecord,
        branch: &BranchRecord,
        system_prompt: Option<String>,
        tools: Vec<ToolSpec>,
        thinking: Option<ThinkingConfig>,
        turn_id: Option<TurnId>,
        settings_revision_id: u64,
        force_compaction: bool,
    ) -> Result<PreparedTurnContext> {
        let (connection, model_id) =
            self.resolve_turn_settings(session.session_id, settings_revision_id)?;
        self.prepare_turn_context_for_resolved_settings(
            session,
            branch,
            connection,
            model_id,
            system_prompt,
            tools,
            thinking,
            turn_id,
            settings_revision_id,
            force_compaction,
        )
    }

    pub(crate) fn prepare_turn_context_for_resolved_settings(
        &self,
        session: &SessionRecord,
        branch: &BranchRecord,
        connection: ConnectionDescriptor,
        model_id: String,
        system_prompt: Option<String>,
        tools: Vec<ToolSpec>,
        thinking: Option<ThinkingConfig>,
        turn_id: Option<TurnId>,
        settings_revision_id: u64,
        force_compaction: bool,
    ) -> Result<PreparedTurnContext> {
        let context_messages = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_context_messages(session.session_id, branch.branch_id)?;
        let context_boundary_seq_id = context_messages
            .iter()
            .filter_map(|record| record.source_seq_id)
            .max();
        let message_sources = context_messages
            .iter()
            .filter_map(|record| {
                Some(bt_core::ContextMessageSourceRef {
                    message_id: record.message.message_id,
                    branch_id: record.source_branch_id?,
                    seq_id: record.source_seq_id?,
                })
            })
            .collect::<Vec<_>>();
        let messages = context_messages
            .into_iter()
            .map(|record| record.message)
            .collect::<Vec<_>>();
        let context = ContextAssembler::new(self.config.clone())?;
        let build_started = Instant::now();
        let output = context.build_request_with_options(
            connection.id.clone(),
            &connection.provider,
            &model_id,
            system_prompt,
            messages,
            tools,
            thinking,
            force_compaction,
        );
        let build_latency_ms =
            u64::try_from(build_started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut compaction = output.compaction.map(|compaction| {
            let mut report = compaction_report_from_context(compaction);
            report.phase = if force_compaction {
                bt_core::ContextCompactionPhase::Manual
            } else {
                bt_core::ContextCompactionPhase::PreTurn
            };
            report.provider = Some(connection.provider.clone());
            report.model = Some(model_id.clone());
            report.context_boundary_seq_id = context_boundary_seq_id;
            report.latency_ms = Some(build_latency_ms);
            if let Some(message_id) = report.first_kept_message_id
                && let Some(source) = message_sources
                    .iter()
                    .find(|source| source.message_id == message_id)
            {
                report.first_kept_branch_id = Some(source.branch_id);
                report.first_kept_seq_id = Some(source.seq_id);
            }
            report
        });
        if let Some(compaction) = &mut compaction {
            let (_, event_id) = self.record_context_compacted(
                session.session_id,
                branch.branch_id,
                compaction,
                turn_id,
            )?;
            compaction.source_event_id = Some(event_id);
        }
        Ok(PreparedTurnContext {
            settings_revision_id,
            connection,
            model_id,
            context_boundary_seq_id,
            message_sources,
            request: output.request,
            compaction,
        })
    }
}
