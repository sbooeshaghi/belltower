// queries.rs keeps one bounded runtime concern out of `runtime.rs`; cross-crate ownership remains unchanged.
use super::support::*;
use super::*;

impl BelltowerRuntime {
    pub fn replay_events_after(
        &self,
        session_id: SessionId,
        after_seq_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<EventEnvelope>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_events_after(session_id, after_seq_id, limit)
    }

    pub fn all_events(&self, session_id: SessionId) -> Result<Vec<EventEnvelope>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_all_events(session_id)
    }

    pub fn events_of_kind(
        &self,
        session_id: SessionId,
        event_kind: &str,
        branch_id: Option<bt_core::BranchId>,
    ) -> Result<Vec<EventEnvelope>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_events_of_kind(session_id, event_kind, branch_id)
    }

    /// The session's current active-turn claim (branch, turn), if a turn owns
    /// the session right now. Indexed projection lookup — never a log replay.
    pub fn active_turn_claim(
        &self,
        session_id: SessionId,
    ) -> Result<Option<(bt_core::BranchId, TurnId)>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .active_turn_claim(session_id)
    }

    pub fn operator_command_events(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
    ) -> Result<Vec<EventEnvelope>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_operator_command_events(session_id, branch_id)
    }

    pub fn branch_messages_page(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        before_seq_id: Option<i64>,
        limit: usize,
    ) -> Result<BranchTranscriptPage<SequencedMessageRecord>> {
        let store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        let page = store.load_branch_messages_page(session_id, branch_id, before_seq_id, limit)?;
        let last_seq_id = store
            .load_session_inspection_metrics(session_id)?
            .last_seq_id;
        Ok(to_runtime_page(page, last_seq_id))
    }

    pub fn branch_operator_commands_page(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        before_seq_id: Option<i64>,
        limit: usize,
    ) -> Result<BranchTranscriptPage<RecordedOperatorCommandRecord>> {
        let store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        let page = store.load_branch_operator_commands_page(
            session_id,
            branch_id,
            before_seq_id,
            limit,
        )?;
        let last_seq_id = store
            .load_session_inspection_metrics(session_id)?
            .last_seq_id;
        Ok(to_runtime_page(page, last_seq_id))
    }

    pub fn search_session_history(
        &self,
        session_id: SessionId,
        branch_id: Option<bt_core::BranchId>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<bt_core::SessionSearchMatch>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .search_session_history(session_id, branch_id, query, limit)
    }

    pub fn raw_chunks(&self, session_id: SessionId, limit: usize) -> Result<Vec<RawChunkRecord>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_raw_chunks(session_id, limit)
    }

    pub fn export_legacy_bundle(&self, session_id: SessionId) -> Result<LegacySessionExportBundle> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .export_legacy_bundle(session_id)
    }

    pub fn turn_raw_chunks_page(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        turn_id: TurnId,
        llm_call_ordinal: Option<u32>,
        before_chunk_id: Option<i64>,
        limit: usize,
    ) -> Result<ChunkPage<RawChunkRecord>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_turn_raw_chunks_page(
                session_id,
                branch_id,
                turn_id,
                llm_call_ordinal,
                before_chunk_id,
                limit,
            )
    }

    pub fn load_session(&self, session_id: SessionId) -> Result<Option<SessionRecord>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_session(session_id)
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionRecord>> {
        let store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        let sessions = store.list_sessions()?;
        let mut visible = Vec::with_capacity(sessions.len());
        for session in sessions {
            if store.session_has_events(session.session_id)? {
                visible.push(session);
            }
        }
        Ok(visible)
    }

    pub fn load_branches(&self, session_id: SessionId) -> Result<Vec<BranchRecord>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_branches(session_id)
    }

    pub fn load_branch(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
    ) -> Result<Option<BranchRecord>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_branch(session_id, branch_id)
    }

    pub fn default_branch(&self, session_id: SessionId) -> Result<Option<BranchRecord>> {
        Ok(self
            .load_branches(session_id)?
            .into_iter()
            .find(|branch| branch.is_default))
    }
}
