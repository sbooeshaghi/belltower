use crate::{CompletionCostReprojector, ReprojectionReport, StoredSessionEvent, apply_migrations};
use bt_core::{
    ApprovalScope, BelltowerError, BranchHead, BranchId, BranchRecord, BudgetConfig, EventEnvelope,
    EventPayload, Message, Result, Role, SessionId, SessionRecord, SessionSettingsSnapshot,
    SessionToolMode, ToolCallId, ToolOperationContext, TurnId, TurnStartSource,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use time::OffsetDateTime;

const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub struct SqliteSessionStore {
    connection: Connection,
}

mod codec;
use codec::{
    approval_status, count_query, format_time, load_session_event_log_from_connection,
    optional_i64_query, parse_branch_record, parse_raw_chunk_record, parse_session_record,
    parse_session_tool_mode, parse_time, queued_message_resolution_status, session_status_to_str,
    session_tool_mode_to_str, span_kind_to_str, steer_resolution_status, storage_error,
    to_sql_conversion,
};
mod admission;
mod paging;
mod projections;
mod search;
use admission::{
    validate_queued_continuation_events, validate_steer_continuation_events,
    validate_turn_admission_events,
};
mod types;
pub use types::{
    ApprovalProjection, ChunkPage, ContextManifestProjection, ContextMessageRecord,
    ContinuationClaim, CostSummaryProjection, PlanProjection, QueuedMessageProjection,
    RawChunkInsert, RawChunkRecord, RecordedOperatorCommandRecord, ResumedTurnRecoveryRecord,
    ReusableApprovalProjection, SequencedMessageRecord, SessionBudgetProjection,
    SessionControlProjection, SessionInspectionMetrics, SessionSettingsRevisionProjection,
    SessionTurnAdmission, SteerProjection, ToolOperationRecoveryRecord, ToolRunProjection,
    TranscriptPage, TurnRecoveryRecord,
};

impl SqliteSessionStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path).map_err(storage_error)?;
        connection
            .busy_timeout(SQLITE_BUSY_TIMEOUT)
            .map_err(storage_error)?;
        apply_migrations(&connection).map_err(storage_error)?;
        Ok(Self { connection })
    }

    pub fn open_in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory().map_err(storage_error)?;
        connection
            .busy_timeout(SQLITE_BUSY_TIMEOUT)
            .map_err(storage_error)?;
        apply_migrations(&connection).map_err(storage_error)?;
        Ok(Self { connection })
    }

    pub fn create_session(&mut self, session: &SessionRecord, branch: &BranchRecord) -> Result<()> {
        let tx = self.connection.transaction().map_err(storage_error)?;
        Self::insert_session_in_tx(&tx, session)?;
        Self::insert_session_settings_revision_in_tx(&tx, session)?;
        Self::insert_branch_in_tx(&tx, branch)?;
        tx.commit().map_err(storage_error)?;
        Ok(())
    }

    pub fn import_session_with_raw_chunks_and_events<F>(
        &mut self,
        session: &SessionRecord,
        branches: &[BranchRecord],
        raw_chunks: &[RawChunkInsert],
        build_events: F,
    ) -> Result<(Vec<i64>, Vec<i64>)>
    where
        F: FnOnce(&[i64]) -> Result<Vec<EventEnvelope>>,
    {
        let tx = self.connection.transaction().map_err(storage_error)?;
        Self::insert_session_in_tx(&tx, session)?;
        Self::insert_session_settings_revision_in_tx(&tx, session)?;
        for branch in branches {
            Self::insert_branch_in_tx(&tx, branch)?;
        }

        let mut raw_chunk_ids = Vec::with_capacity(raw_chunks.len());
        for raw_chunk in raw_chunks {
            raw_chunk_ids.push(Self::append_raw_chunk_in_tx(&tx, raw_chunk)?);
        }

        let events = build_events(&raw_chunk_ids)?;
        let mut seq_ids = Vec::with_capacity(events.len());
        for event in &events {
            seq_ids.push(Self::append_event_in_tx(&tx, event)?);
        }

        tx.commit().map_err(storage_error)?;
        Ok((raw_chunk_ids, seq_ids))
    }

    pub fn create_branch(&mut self, branch: &BranchRecord) -> Result<()> {
        Self::insert_branch_in_connection(&self.connection, branch)?;
        Ok(())
    }

    pub fn create_branch_with_event_and_default(
        &mut self,
        branch: &BranchRecord,
        event: &EventEnvelope,
    ) -> Result<i64> {
        let tx = self.connection.transaction().map_err(storage_error)?;
        Self::insert_branch_in_tx(&tx, branch)?;
        let seq_id = Self::append_event_in_tx(&tx, event)?;
        Self::set_default_branch_in_tx(&tx, branch.session_id, branch.branch_id)?;
        tx.commit().map_err(storage_error)?;
        Ok(seq_id)
    }

    pub fn append_event(&mut self, event: &EventEnvelope) -> Result<i64> {
        let tx = self.connection.transaction().map_err(storage_error)?;
        let seq_id = Self::append_event_in_tx(&tx, event)?;
        tx.commit().map_err(storage_error)?;
        Ok(seq_id)
    }

    pub fn append_events(&mut self, events: &[EventEnvelope]) -> Result<Vec<i64>> {
        let tx = self.connection.transaction().map_err(storage_error)?;
        let mut seq_ids = Vec::with_capacity(events.len());
        for event in events {
            seq_ids.push(Self::append_event_in_tx(&tx, event)?);
        }
        tx.commit().map_err(storage_error)?;
        Ok(seq_ids)
    }

    pub fn admit_turn_or_queue(
        &mut self,
        session_id: SessionId,
        expected_settings_revision_id: u64,
        cancel_clear_event: &EventEnvelope,
        started_events: &[EventEnvelope],
        queued_events: &[EventEnvelope],
    ) -> Result<SessionTurnAdmission> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let current_settings_revision_id = tx
            .query_row(
                "SELECT settings_revision_id FROM sessions WHERE session_id = ?1",
                params![session_id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(storage_error)?
            .ok_or_else(|| BelltowerError::InvalidState("session not found".to_owned()))?
            .max(1) as u64;
        if current_settings_revision_id != expected_settings_revision_id {
            return Ok(SessionTurnAdmission::RetryWithSettings {
                settings_revision_id: current_settings_revision_id,
            });
        }
        validate_turn_admission_events(
            session_id,
            expected_settings_revision_id,
            cancel_clear_event,
            started_events,
            queued_events,
        )?;

        if Self::session_has_pending_work_in_tx(&tx, session_id)? {
            let seq_ids = Self::append_events_in_tx(&tx, queued_events)?;
            let position = tx
                .query_row(
                    "SELECT COUNT(*) FROM queued_message_projection
                     WHERE session_id = ?1 AND status = 'pending'",
                    params![session_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(storage_error)?
                .max(0) as usize;
            tx.commit().map_err(storage_error)?;
            return Ok(SessionTurnAdmission::Queued { seq_ids, position });
        }

        let mut seq_ids = Vec::with_capacity(started_events.len() + 1);
        if Self::cancel_requested_in_tx(&tx, session_id)? {
            seq_ids.push(Self::append_event_in_tx(&tx, cancel_clear_event)?);
        }
        for event in started_events {
            seq_ids.push(Self::append_event_in_tx(&tx, event)?);
        }
        tx.commit().map_err(storage_error)?;
        Ok(SessionTurnAdmission::Started { seq_ids })
    }

    pub fn claim_queued_continuation(
        &mut self,
        session_id: SessionId,
        queue_event_id: bt_core::EventId,
        events: &[EventEnvelope],
    ) -> Result<ContinuationClaim> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        if Self::cancel_requested_in_tx(&tx, session_id)? {
            return Ok(ContinuationClaim::CancelPending);
        }
        if Self::active_turn_exists_in_tx(&tx, session_id)? {
            return Ok(ContinuationClaim::Busy);
        }
        let next_queue = tx
            .query_row(
                "SELECT queue_event_id, settings_revision_id FROM queued_message_projection
                 WHERE session_id = ?1 AND status = 'pending'
                 ORDER BY source_seq ASC LIMIT 1",
                params![session_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?.max(1) as u64,
                    ))
                },
            )
            .optional()
            .map_err(storage_error)?;
        let expected_queue_event_id = queue_event_id.to_string();
        let Some((next_queue_event_id, settings_revision_id)) = next_queue else {
            return Ok(ContinuationClaim::Stale);
        };
        if next_queue_event_id != expected_queue_event_id {
            return Ok(ContinuationClaim::Stale);
        }
        validate_queued_continuation_events(
            session_id,
            queue_event_id,
            settings_revision_id,
            events,
        )?;
        let seq_ids = Self::append_events_in_tx(&tx, events)?;
        tx.commit().map_err(storage_error)?;
        Ok(ContinuationClaim::Claimed { seq_ids })
    }

    pub fn claim_steer_continuation(
        &mut self,
        session_id: SessionId,
        branch_id: BranchId,
        steer_event_ids: &[bt_core::EventId],
        events: &[EventEnvelope],
    ) -> Result<ContinuationClaim> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        if Self::cancel_requested_in_tx(&tx, session_id)? {
            return Ok(ContinuationClaim::CancelPending);
        }
        if Self::active_turn_exists_in_tx(&tx, session_id)? {
            return Ok(ContinuationClaim::Busy);
        }
        let pending_sources = {
            let mut statement = tx
                .prepare(
                    "SELECT steer_event_id, settings_revision_id FROM steer_projection
                     WHERE session_id = ?1 AND branch_id = ?2 AND status = 'pending'
                     ORDER BY source_seq ASC",
                )
                .map_err(storage_error)?;
            let rows = statement
                .query_map(
                    params![session_id.to_string(), branch_id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?.max(1) as u64,
                        ))
                    },
                )
                .map_err(storage_error)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(storage_error)?
        };
        let pending_ids = pending_sources
            .iter()
            .map(|(event_id, _)| event_id.clone())
            .collect::<Vec<_>>();
        let expected_ids = steer_event_ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if pending_ids != expected_ids {
            return Ok(ContinuationClaim::Stale);
        }
        let Some((_, settings_revision_id)) = pending_sources.last() else {
            return Ok(ContinuationClaim::Stale);
        };
        validate_steer_continuation_events(
            session_id,
            branch_id,
            steer_event_ids,
            *settings_revision_id,
            events,
        )?;
        let seq_ids = Self::append_events_in_tx(&tx, events)?;
        tx.commit().map_err(storage_error)?;
        Ok(ContinuationClaim::Claimed { seq_ids })
    }

    pub fn append_raw_chunk_with_event<F>(
        &mut self,
        session_id: SessionId,
        branch_id: BranchId,
        turn_id: Option<TurnId>,
        llm_call_ordinal: Option<u32>,
        provider: &str,
        stream_name: &str,
        content: &[u8],
        build_event: F,
    ) -> Result<(i64, EventEnvelope, i64)>
    where
        F: FnOnce(i64) -> Result<EventEnvelope>,
    {
        let tx = self.connection.transaction().map_err(storage_error)?;
        tx.execute(
            "INSERT INTO raw_chunks (session_id, branch_id, turn_id, llm_call_ordinal, event_id, provider, stream_name, content, received_at)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7, ?8)",
            params![
                session_id.to_string(),
                branch_id.to_string(),
                turn_id.map(|id| id.to_string()),
                llm_call_ordinal.map(i64::from),
                provider,
                stream_name,
                content,
                format_time(OffsetDateTime::now_utc())?
            ],
        )
        .map_err(storage_error)?;
        let chunk_id = tx.last_insert_rowid();
        let event = build_event(chunk_id)?;
        tx.execute(
            "UPDATE raw_chunks SET event_id = ?1 WHERE chunk_id = ?2",
            params![event.event_id.to_string(), chunk_id],
        )
        .map_err(storage_error)?;
        let seq_id = Self::append_event_in_tx(&tx, &event)?;
        tx.commit().map_err(storage_error)?;
        Ok((chunk_id, event, seq_id))
    }

    pub fn append_raw_chunk(
        &mut self,
        session_id: SessionId,
        branch_id: BranchId,
        turn_id: Option<TurnId>,
        llm_call_ordinal: Option<u32>,
        event_id: Option<String>,
        provider: &str,
        stream_name: &str,
        content: &[u8],
    ) -> Result<i64> {
        self.connection
            .execute(
            "INSERT INTO raw_chunks (session_id, branch_id, turn_id, llm_call_ordinal, event_id, provider, stream_name, content, received_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                session_id.to_string(),
                branch_id.to_string(),
                turn_id.map(|id| id.to_string()),
                llm_call_ordinal.map(i64::from),
                event_id,
                provider,
                stream_name,
                content,
                format_time(OffsetDateTime::now_utc())?
            ],
        )
            .map_err(storage_error)?;
        Ok(self.connection.last_insert_rowid())
    }

    pub fn load_raw_chunks(
        &self,
        session_id: SessionId,
        limit: usize,
    ) -> Result<Vec<RawChunkRecord>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT chunk_id, session_id, branch_id, turn_id, llm_call_ordinal, event_id, provider, stream_name, content, received_at
             FROM raw_chunks
             WHERE session_id = ?1
             ORDER BY chunk_id ASC
             LIMIT ?2",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![session_id.to_string(), limit as i64],
                parse_raw_chunk_record,
            )
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub fn load_all_raw_chunks(&self, session_id: SessionId) -> Result<Vec<RawChunkRecord>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT chunk_id, session_id, branch_id, turn_id, llm_call_ordinal, event_id, provider, stream_name, content, received_at
                 FROM raw_chunks
                 WHERE session_id = ?1
                 ORDER BY chunk_id ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![session_id.to_string()], parse_raw_chunk_record)
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub fn load_turn_raw_chunks_page(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        turn_id: TurnId,
        llm_call_ordinal: Option<u32>,
        before_chunk_id: Option<i64>,
        limit: usize,
    ) -> Result<ChunkPage<RawChunkRecord>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT chunk_id, session_id, branch_id, turn_id, llm_call_ordinal, event_id, provider, stream_name, content, received_at
                 FROM raw_chunks
                 WHERE session_id = ?1 AND branch_id = ?2 AND turn_id = ?3
                   AND (?4 IS NULL OR llm_call_ordinal = ?4)
                   AND (?5 IS NULL OR chunk_id < ?5)
                 ORDER BY chunk_id DESC
                 LIMIT ?6",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![
                    session_id.to_string(),
                    branch_id.to_string(),
                    turn_id.to_string(),
                    llm_call_ordinal.map(i64::from),
                    before_chunk_id,
                    (limit.saturating_add(1)) as i64
                ],
                parse_raw_chunk_record,
            )
            .map_err(storage_error)?;
        let mut items = rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        let has_more_before = items.len() > limit;
        if has_more_before {
            items.pop();
        }
        items.reverse();
        let oldest_chunk_id = items.first().map(|chunk| chunk.chunk_id);
        let newest_chunk_id = items.last().map(|chunk| chunk.chunk_id);
        Ok(ChunkPage {
            items,
            oldest_chunk_id,
            newest_chunk_id,
            has_more_before,
        })
    }

    fn append_event_in_tx(tx: &Transaction<'_>, event: &EventEnvelope) -> Result<i64> {
        let event_json = serde_json::to_string(event)?;
        tx.execute(
            "INSERT INTO events (
                event_id, session_id, branch_id, span_id, parent_span_id, span_kind, event_kind, event_json, occurred_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                event.event_id.to_string(),
                event.session_id.to_string(),
                event.branch_id.to_string(),
                event.span_id.to_string(),
                event.parent_span_id.map(|value| value.to_string()),
                span_kind_to_str(&event.span_kind),
                event.kind(),
                event_json,
                format_time(event.occurred_at)?,
            ],
        )
        .map_err(storage_error)?;
        let seq_id = tx.last_insert_rowid();
        Self::refresh_projections(tx, event, seq_id)?;
        Ok(seq_id)
    }

    fn append_events_in_tx(tx: &Transaction<'_>, events: &[EventEnvelope]) -> Result<Vec<i64>> {
        events
            .iter()
            .map(|event| Self::append_event_in_tx(tx, event))
            .collect()
    }

    fn active_turn_exists_in_tx(tx: &Transaction<'_>, session_id: SessionId) -> Result<bool> {
        tx.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM active_turn_projection WHERE session_id = ?1
            )",
            params![session_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value != 0)
        .map_err(storage_error)
    }

    fn cancel_requested_in_tx(tx: &Transaction<'_>, session_id: SessionId) -> Result<bool> {
        tx.query_row(
            "SELECT COALESCE((
                SELECT cancel_requested FROM session_control_projection WHERE session_id = ?1
            ), 0)",
            params![session_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value != 0)
        .map_err(storage_error)
    }

    fn session_has_pending_work_in_tx(tx: &Transaction<'_>, session_id: SessionId) -> Result<bool> {
        tx.query_row(
            "SELECT
                EXISTS(SELECT 1 FROM active_turn_projection WHERE session_id = ?1)
                OR EXISTS(SELECT 1 FROM approval_projection
                          WHERE session_id = ?1 AND status = 'pending')
                OR EXISTS(SELECT 1 FROM tool_run_projection
                          WHERE session_id = ?1 AND tool_name = 'ask' AND status = 'requested')
                OR EXISTS(SELECT 1 FROM queued_message_projection
                          WHERE session_id = ?1 AND status = 'pending')
                OR EXISTS(SELECT 1 FROM steer_projection
                          WHERE session_id = ?1 AND status = 'pending')",
            params![session_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value != 0)
        .map_err(storage_error)
    }

    fn append_raw_chunk_in_tx(tx: &Transaction<'_>, raw_chunk: &RawChunkInsert) -> Result<i64> {
        tx.execute(
            "INSERT INTO raw_chunks (session_id, branch_id, turn_id, llm_call_ordinal, event_id, provider, stream_name, content, received_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                raw_chunk.session_id.to_string(),
                raw_chunk.branch_id.to_string(),
                raw_chunk.turn_id.map(|id| id.to_string()),
                raw_chunk.llm_call_ordinal.map(i64::from),
                raw_chunk.event_id.as_deref(),
                raw_chunk.provider.as_str(),
                raw_chunk.stream_name.as_str(),
                raw_chunk.content.as_slice(),
                format_time(OffsetDateTime::now_utc())?
            ],
        )
        .map_err(storage_error)?;
        Ok(tx.last_insert_rowid())
    }

    fn insert_session_in_tx(tx: &Transaction<'_>, session: &SessionRecord) -> Result<()> {
        tx.execute(
            "INSERT INTO sessions (
                session_id, project_root, connection_id, model_id, tool_mode, settings_revision_id, created_at, updated_at, status, display_name, objective, parent_session_id, parent_branch_id, parent_turn_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                session.session_id.to_string(),
                session.project_root.as_str(),
                session.connection_id.to_string(),
                session.model_id,
                session_tool_mode_to_str(session.tool_mode),
                session.settings_revision_id as i64,
                format_time(session.created_at)?,
                format_time(session.updated_at)?,
                session_status_to_str(&session.status),
                session.display_name,
                session.objective,
                session.parent_session_id.map(|id| id.to_string()),
                session.parent_branch_id.map(|id| id.to_string()),
                session.parent_turn_id.map(|id| id.to_string()),
            ],
        )
        .map_err(storage_error)?;
        Ok(())
    }

    fn insert_session_settings_revision_in_tx(
        tx: &Transaction<'_>,
        session: &SessionRecord,
    ) -> Result<()> {
        tx.execute(
            "INSERT INTO session_settings_revision_projection (
                session_id, settings_revision_id, connection_id, model_id, tool_mode, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                session.session_id.to_string(),
                session.settings_revision_id as i64,
                session.connection_id.to_string(),
                session.model_id,
                session_tool_mode_to_str(session.tool_mode),
                format_time(session.updated_at)?,
            ],
        )
        .map_err(storage_error)?;
        Ok(())
    }

    fn insert_branch_in_connection(connection: &Connection, branch: &BranchRecord) -> Result<()> {
        connection
            .execute(
                "INSERT INTO branches (
                    branch_id, session_id, parent_branch_id, parent_event_id, head_event_id, summary, created_at, is_default
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    branch.branch_id.to_string(),
                    branch.session_id.to_string(),
                    branch.parent_branch_id.map(|id| id.to_string()),
                    branch.parent_event_id.map(|id| id.to_string()),
                    branch.head_event_id.map(|id| id.to_string()),
                    branch.summary,
                    format_time(branch.created_at)?,
                    branch.is_default,
                ],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    fn insert_branch_in_tx(tx: &Transaction<'_>, branch: &BranchRecord) -> Result<()> {
        tx.execute(
            "INSERT INTO branches (
                branch_id, session_id, parent_branch_id, parent_event_id, head_event_id, summary, created_at, is_default
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                branch.branch_id.to_string(),
                branch.session_id.to_string(),
                branch.parent_branch_id.map(|id| id.to_string()),
                branch.parent_event_id.map(|id| id.to_string()),
                branch.head_event_id.map(|id| id.to_string()),
                branch.summary,
                format_time(branch.created_at)?,
                branch.is_default,
            ],
        )
        .map_err(storage_error)?;
        Ok(())
    }

    pub fn load_session(&self, session_id: SessionId) -> Result<Option<SessionRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT session_id, project_root, connection_id, model_id, tool_mode, settings_revision_id, created_at, updated_at, status, display_name, objective, parent_session_id, parent_branch_id, parent_turn_id
             FROM sessions
             WHERE session_id = ?1",
        ).map_err(storage_error)?;

        statement
            .query_row(params![session_id.to_string()], parse_session_record)
            .optional()
            .map_err(storage_error)
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT session_id, project_root, connection_id, model_id, tool_mode, settings_revision_id, created_at, updated_at, status, display_name, objective, parent_session_id, parent_branch_id, parent_turn_id
             FROM sessions
             ORDER BY created_at ASC",
        ).map_err(storage_error)?;
        let rows = statement
            .query_map([], parse_session_record)
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub fn session_has_events(&self, session_id: SessionId) -> Result<bool> {
        let exists: i64 = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM events WHERE session_id = ?1 LIMIT 1)",
                params![session_id.to_string()],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        Ok(exists != 0)
    }

    pub fn update_session(
        &mut self,
        session_id: SessionId,
        connection_id: &bt_core::ConnectionId,
        model_id: Option<&str>,
        tool_mode: SessionToolMode,
        settings_revision_id: u64,
    ) -> Result<()> {
        let updated_at = OffsetDateTime::now_utc();
        let tx = self.connection.transaction().map_err(storage_error)?;
        tx.execute(
            "UPDATE sessions
             SET connection_id = ?1, model_id = ?2, tool_mode = ?3, settings_revision_id = ?4, updated_at = ?5
             WHERE session_id = ?6",
            params![
                connection_id.to_string(),
                model_id,
                session_tool_mode_to_str(tool_mode),
                settings_revision_id as i64,
                format_time(updated_at)?,
                session_id.to_string(),
            ],
        )
        .map_err(storage_error)?;
        tx.execute(
            "INSERT INTO session_settings_revision_projection (
                session_id, settings_revision_id, connection_id, model_id, tool_mode, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(session_id, settings_revision_id) DO UPDATE SET
                connection_id = excluded.connection_id,
                model_id = excluded.model_id,
                tool_mode = excluded.tool_mode,
                updated_at = excluded.updated_at",
            params![
                session_id.to_string(),
                settings_revision_id as i64,
                connection_id.to_string(),
                model_id,
                session_tool_mode_to_str(tool_mode),
                format_time(updated_at)?,
            ],
        )
        .map_err(storage_error)?;
        tx.commit().map_err(storage_error)?;
        Ok(())
    }

    pub fn update_session_parent(
        &mut self,
        session_id: SessionId,
        parent_session_id: SessionId,
        parent_branch_id: BranchId,
        parent_turn_id: Option<bt_core::TurnId>,
    ) -> Result<()> {
        self.connection
            .execute(
                "UPDATE sessions
                 SET parent_session_id = ?1, parent_branch_id = ?2, parent_turn_id = ?3, updated_at = ?4
                 WHERE session_id = ?5",
                params![
                    parent_session_id.to_string(),
                    parent_branch_id.to_string(),
                    parent_turn_id.map(|turn_id| turn_id.to_string()),
                    format_time(OffsetDateTime::now_utc())?,
                    session_id.to_string(),
                ],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn load_branches(&self, session_id: SessionId) -> Result<Vec<BranchRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT branch_id, session_id, parent_branch_id, parent_event_id, head_event_id, summary, created_at, is_default
             FROM branches
             WHERE session_id = ?1
             ORDER BY created_at ASC",
        ).map_err(storage_error)?;
        let rows = statement
            .query_map(params![session_id.to_string()], parse_branch_record)
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub fn load_child_sessions(&self, parent_session_id: SessionId) -> Result<Vec<SessionRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT session_id, project_root, connection_id, model_id, tool_mode, settings_revision_id, created_at, updated_at, status, display_name, objective, parent_session_id, parent_branch_id, parent_turn_id
             FROM sessions
             WHERE parent_session_id = ?1
             ORDER BY created_at ASC",
        ).map_err(storage_error)?;
        let rows = statement
            .query_map(params![parent_session_id.to_string()], parse_session_record)
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub fn load_branch(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
    ) -> Result<Option<BranchRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT branch_id, session_id, parent_branch_id, parent_event_id, head_event_id, summary, created_at, is_default
             FROM branches
             WHERE session_id = ?1 AND branch_id = ?2",
        ).map_err(storage_error)?;
        statement
            .query_row(
                params![session_id.to_string(), branch_id.to_string()],
                parse_branch_record,
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn load_events_after(
        &self,
        session_id: SessionId,
        after_seq_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<EventEnvelope>> {
        let after = after_seq_id.unwrap_or(0);
        let mut statement = self
            .connection
            .prepare(
                "SELECT event_json, seq_id
             FROM events
             WHERE session_id = ?1 AND seq_id > ?2
             ORDER BY seq_id ASC
             LIMIT ?3",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![session_id.to_string(), after, limit as i64],
                |row| {
                    let raw: String = row.get(0)?;
                    let seq_id: i64 = row.get(1)?;
                    let mut event: EventEnvelope =
                        serde_json::from_str(&raw).map_err(to_sql_conversion)?;
                    event.seq_id = Some(seq_id);
                    Ok(event)
                },
            )
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub fn load_event(
        &self,
        session_id: SessionId,
        event_id: bt_core::EventId,
    ) -> Result<Option<EventEnvelope>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT event_json, seq_id
                 FROM events
                 WHERE session_id = ?1 AND event_id = ?2",
            )
            .map_err(storage_error)?;
        statement
            .query_row(
                params![session_id.to_string(), event_id.to_string()],
                |row| {
                    let raw: String = row.get(0)?;
                    let seq_id: i64 = row.get(1)?;
                    let mut event: EventEnvelope =
                        serde_json::from_str(&raw).map_err(to_sql_conversion)?;
                    event.seq_id = Some(seq_id);
                    Ok(event)
                },
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn load_all_events(&self, session_id: SessionId) -> Result<Vec<EventEnvelope>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT event_json, seq_id
                 FROM events
                 WHERE session_id = ?1
                 ORDER BY seq_id ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![session_id.to_string()], |row| {
                let raw: String = row.get(0)?;
                let seq_id: i64 = row.get(1)?;
                let mut event: EventEnvelope =
                    serde_json::from_str(&raw).map_err(to_sql_conversion)?;
                event.seq_id = Some(seq_id);
                Ok(event)
            })
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub fn load_session_event_log(&self, session_id: SessionId) -> Result<Vec<StoredSessionEvent>> {
        load_session_event_log_from_connection(&self.connection, session_id)
    }

    pub fn load_unfinished_resumed_turns(&self) -> Result<Vec<ResumedTurnRecoveryRecord>> {
        let mut records = self
            .load_unfinished_turns()?
            .into_iter()
            .filter(|record| {
                matches!(
                    record.source,
                    TurnStartSource::ApprovalResume | TurnStartSource::InputResume
                )
            })
            .map(|record| ResumedTurnRecoveryRecord {
                session_id: record.session_id,
                branch_id: record.branch_id,
                turn_id: record.turn_id,
                provider: record.provider,
                model: record.model,
                started_seq_id: record.started_seq_id,
            })
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.started_seq_id);
        Ok(records)
    }

    pub fn load_unfinished_turns(&self) -> Result<Vec<TurnRecoveryRecord>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT event_json, seq_id
                 FROM events
                 WHERE event_kind IN ('turn.started', 'turn.finished')
                 ORDER BY seq_id ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], |row| {
                let raw: String = row.get(0)?;
                let seq_id: i64 = row.get(1)?;
                let mut event: EventEnvelope =
                    serde_json::from_str(&raw).map_err(to_sql_conversion)?;
                event.seq_id = Some(seq_id);
                Ok((event, seq_id))
            })
            .map_err(storage_error)?;

        let mut unfinished = HashMap::new();
        for row in rows {
            let (event, seq_id) = row.map_err(storage_error)?;
            match event.payload {
                EventPayload::TurnStarted {
                    turn_id,
                    provider,
                    model,
                    source,
                    resumed_from_call_id,
                    ..
                } => {
                    unfinished.insert(
                        turn_id,
                        TurnRecoveryRecord {
                            session_id: event.session_id,
                            branch_id: event.branch_id,
                            turn_id,
                            provider,
                            model,
                            source,
                            resumed_from_call_id,
                            started_seq_id: seq_id,
                        },
                    );
                }
                EventPayload::TurnFinished { turn_id, .. } => {
                    unfinished.remove(&turn_id);
                }
                _ => {}
            }
        }

        let mut records = unfinished.into_values().collect::<Vec<_>>();
        records.sort_by_key(|record| record.started_seq_id);
        Ok(records)
    }

    pub fn load_messages(
        &self,
        session_id: SessionId,
        branch_id: Option<BranchId>,
    ) -> Result<Vec<Message>> {
        if let Some(branch_id) = branch_id {
            self.load_messages_for_branch(session_id, branch_id)
        } else {
            self.load_session_messages_flat(session_id)
        }
    }

    pub fn load_context_messages(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
    ) -> Result<Vec<ContextMessageRecord>> {
        self.load_context_messages_for_branch(session_id, branch_id)
    }

    pub fn load_local_branch_messages(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
    ) -> Result<Vec<Message>> {
        self.load_messages_for_exact_branch(session_id, branch_id, None)
    }

    pub fn load_operator_command_events(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
    ) -> Result<Vec<EventEnvelope>> {
        let lineage = self.branch_lineage(session_id, branch_id)?;
        let mut events = Vec::new();
        for index in 0..lineage.len() {
            let branch = &lineage[index];
            let upper_bound = lineage
                .get(index + 1)
                .and_then(|child| child.parent_event_id);
            events.extend(self.load_operator_command_events_for_exact_branch(
                session_id,
                branch.branch_id,
                upper_bound,
            )?);
        }
        Ok(events)
    }

    pub fn load_turn_context_manifests(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
    ) -> Result<Vec<ContextManifestProjection>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT session_id, branch_id, turn_id, llm_call_ordinal, provider, model,
                        settings_revision_id, context_boundary_seq_id, message_count, tool_count,
                        attachment_count, compacted, manifest_json, recorded_event_id,
                        recorded_seq_id, recorded_at
                 FROM context_manifest_projection
                 WHERE session_id = ?1 AND turn_id = ?2
                 ORDER BY llm_call_ordinal ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![session_id.to_string(), turn_id.to_string()],
                |row| {
                    let session_id: String = row.get(0)?;
                    let branch_id: String = row.get(1)?;
                    let turn_id: String = row.get(2)?;
                    let llm_call_ordinal: i64 = row.get(3)?;
                    let settings_revision_id: i64 = row.get(6)?;
                    let message_count: i64 = row.get(8)?;
                    let tool_count: i64 = row.get(9)?;
                    let attachment_count: i64 = row.get(10)?;
                    let compacted: i64 = row.get(11)?;
                    let manifest_json: String = row.get(12)?;
                    let recorded_event_id: String = row.get(13)?;
                    let recorded_at: String = row.get(15)?;
                    Ok(ContextManifestProjection {
                        session_id: session_id.parse().map_err(to_sql_conversion)?,
                        branch_id: branch_id.parse().map_err(to_sql_conversion)?,
                        turn_id: turn_id.parse().map_err(to_sql_conversion)?,
                        llm_call_ordinal: u32::try_from(llm_call_ordinal)
                            .map_err(to_sql_conversion)?,
                        provider: row.get(4)?,
                        model: row.get(5)?,
                        settings_revision_id: u64::try_from(settings_revision_id)
                            .map_err(to_sql_conversion)?,
                        context_boundary_seq_id: row.get(7)?,
                        message_count: u32::try_from(message_count).map_err(to_sql_conversion)?,
                        tool_count: u32::try_from(tool_count).map_err(to_sql_conversion)?,
                        attachment_count: u32::try_from(attachment_count)
                            .map_err(to_sql_conversion)?,
                        compacted: compacted != 0,
                        manifest: serde_json::from_str(&manifest_json)
                            .map_err(to_sql_conversion)?,
                        recorded_event_id: recorded_event_id.parse().map_err(to_sql_conversion)?,
                        recorded_seq_id: row.get(14)?,
                        recorded_at: parse_time(recorded_at)?,
                    })
                },
            )
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub fn load_branch_messages_page(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        before_seq_id: Option<i64>,
        limit: usize,
    ) -> Result<TranscriptPage<SequencedMessageRecord>> {
        let lineage = self.branch_lineage(session_id, branch_id)?;
        let summary = lineage
            .last()
            .and_then(|branch| branch.summary.as_ref())
            .filter(|summary| !summary.trim().is_empty())
            .map(|summary| {
                Message::text(Role::System, format!("Branch handoff summary:\n{summary}"))
            });
        let lineage_bounds = self.branch_lineage_bounds(session_id, branch_id)?;

        let (mut items, oldest_seq_id, newest_seq_id, has_more_before) = self.load_page_rows(
            "SELECT e.seq_id, mp.content_json
             FROM message_projection mp
             JOIN events e ON e.event_id = mp.event_id
             WHERE mp.session_id = ?1 AND {lineage_filter}
               AND (?{before_idx} IS NULL OR e.seq_id < ?{before_idx})
             ORDER BY e.seq_id DESC
             LIMIT ?{limit_idx}",
            session_id,
            &lineage_bounds,
            before_seq_id,
            limit,
            |row| {
                let seq_id: i64 = row.get(0)?;
                let raw: String = row.get(1)?;
                Ok(SequencedMessageRecord {
                    seq_id,
                    message: serde_json::from_str(&raw).map_err(to_sql_conversion)?,
                })
            },
        )?;

        if !has_more_before && let Some(summary) = summary {
            items.insert(
                0,
                SequencedMessageRecord {
                    seq_id: 0,
                    message: summary,
                },
            );
        }

        Ok(TranscriptPage {
            items,
            oldest_seq_id,
            newest_seq_id,
            has_more_before,
        })
    }

    pub fn load_branch_operator_commands_page(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        before_seq_id: Option<i64>,
        limit: usize,
    ) -> Result<TranscriptPage<RecordedOperatorCommandRecord>> {
        let lineage_bounds = self.branch_lineage_bounds(session_id, branch_id)?;
        let (items, oldest_seq_id, newest_seq_id, has_more_before) = self.load_page_rows(
            "SELECT e.seq_id, e.occurred_at, e.event_json
             FROM events e
             WHERE e.session_id = ?1 AND e.event_kind = 'operator.command.recorded'
               AND {lineage_filter}
               AND (?{before_idx} IS NULL OR e.seq_id < ?{before_idx})
             ORDER BY e.seq_id DESC
             LIMIT ?{limit_idx}",
            session_id,
            &lineage_bounds,
            before_seq_id,
            limit,
            |row| {
                let seq_id: i64 = row.get(0)?;
                let occurred_at = parse_time(row.get::<_, String>(1)?)?;
                let raw: String = row.get(2)?;
                let mut event: EventEnvelope =
                    serde_json::from_str(&raw).map_err(to_sql_conversion)?;
                event.seq_id = Some(seq_id);
                match event.payload {
                    EventPayload::OperatorCommandRecorded {
                        command_type,
                        raw_input,
                        output,
                        success,
                    } => Ok(RecordedOperatorCommandRecord {
                        seq_id,
                        occurred_at,
                        command_type,
                        raw_input,
                        output,
                        success,
                    }),
                    _ => Err(to_sql_conversion(std::io::Error::other(
                        "expected operator command event",
                    ))),
                }
            },
        )?;

        Ok(TranscriptPage {
            items,
            oldest_seq_id,
            newest_seq_id,
            has_more_before,
        })
    }

    pub fn set_default_branch(&mut self, session_id: SessionId, branch_id: BranchId) -> Result<()> {
        let tx = self.connection.transaction().map_err(storage_error)?;
        Self::set_default_branch_in_tx(&tx, session_id, branch_id)?;
        tx.commit().map_err(storage_error)?;
        Ok(())
    }

    pub fn set_default_branch_with_event(
        &mut self,
        session_id: SessionId,
        branch_id: BranchId,
        event: &EventEnvelope,
    ) -> Result<i64> {
        let tx = self.connection.transaction().map_err(storage_error)?;
        Self::set_default_branch_in_tx(&tx, session_id, branch_id)?;
        let seq_id = Self::append_event_in_tx(&tx, event)?;
        tx.commit().map_err(storage_error)?;
        Ok(seq_id)
    }

    fn set_default_branch_in_tx(
        tx: &Transaction<'_>,
        session_id: SessionId,
        branch_id: BranchId,
    ) -> Result<()> {
        let branch_exists = tx
            .query_row(
                "SELECT 1 FROM branches WHERE session_id = ?1 AND branch_id = ?2",
                params![session_id.to_string(), branch_id.to_string()],
                |_| Ok(()),
            )
            .optional()
            .map_err(storage_error)?
            .is_some();
        if !branch_exists {
            return Err(BelltowerError::InvalidState(format!(
                "branch {branch_id} not found in session {session_id}"
            )));
        }
        tx.execute(
            "UPDATE branches SET is_default = CASE WHEN branch_id = ?2 THEN 1 ELSE 0 END WHERE session_id = ?1",
            params![session_id.to_string(), branch_id.to_string()],
        )
        .map_err(storage_error)?;
        Ok(())
    }

    pub fn update_branch_summary(
        &mut self,
        branch_id: BranchId,
        summary: Option<&str>,
    ) -> Result<()> {
        self.connection
            .execute(
                "UPDATE branches SET summary = ?1 WHERE branch_id = ?2",
                params![summary, branch_id.to_string()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn load_approvals(&self, session_id: SessionId) -> Result<Vec<ApprovalProjection>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT call_id, session_id, tool_name, status, request_fingerprint, request_snapshot_json, decision_json, resolution_json, updated_at
             FROM approval_projection
             WHERE session_id = ?1
             ORDER BY updated_at ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![session_id.to_string()], |row| {
                let request_snapshot_json: Option<String> = row.get(5)?;
                let decision_json: Option<String> = row.get(6)?;
                let resolution_json: Option<String> = row.get(7)?;
                Ok(ApprovalProjection {
                    call_id: ToolCallId::new(row.get::<_, String>(0)?),
                    session_id: SessionId(
                        row.get::<_, String>(1)?
                            .parse()
                            .map_err(to_sql_conversion)?,
                    ),
                    tool_name: row.get(2)?,
                    status: row.get(3)?,
                    request_fingerprint: row.get(4)?,
                    request_snapshot: request_snapshot_json
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(to_sql_conversion)?,
                    decision: decision_json
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(to_sql_conversion)?,
                    resolution: resolution_json
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(to_sql_conversion)?,
                    updated_at: parse_time(row.get::<_, String>(8)?)?,
                })
            })
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub fn load_session_control(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SessionControlProjection>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT session_id, cancel_requested, updated_at
                 FROM session_control_projection
                 WHERE session_id = ?1",
            )
            .map_err(storage_error)?;
        statement
            .query_row(params![session_id.to_string()], |row| {
                Ok(SessionControlProjection {
                    session_id: SessionId(
                        row.get::<_, String>(0)?
                            .parse()
                            .map_err(to_sql_conversion)?,
                    ),
                    cancel_requested: row.get::<_, i64>(1)? != 0,
                    updated_at: parse_time(row.get::<_, String>(2)?)?,
                })
            })
            .optional()
            .map_err(storage_error)
    }

    pub fn load_pending_queued_messages(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<QueuedMessageProjection>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT queue_event_id, session_id, branch_id, message_json, settings_revision_id, status, source_seq, enqueued_at, resolved_at
                 FROM queued_message_projection
                 WHERE session_id = ?1 AND status = 'pending'
                 ORDER BY source_seq ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![session_id.to_string()], |row| {
                let resolved_at: Option<String> = row.get(8)?;
                Ok(QueuedMessageProjection {
                    queue_event_id: row
                        .get::<_, String>(0)?
                        .parse()
                        .map_err(to_sql_conversion)?,
                    session_id: SessionId(
                        row.get::<_, String>(1)?
                            .parse()
                            .map_err(to_sql_conversion)?,
                    ),
                    branch_id: row
                        .get::<_, String>(2)?
                        .parse()
                        .map_err(to_sql_conversion)?,
                    message: serde_json::from_str(&row.get::<_, String>(3)?)
                        .map_err(to_sql_conversion)?,
                    settings_revision_id: row.get::<_, i64>(4)?.max(1) as u64,
                    status: row.get(5)?,
                    source_seq: row.get(6)?,
                    enqueued_at: parse_time(row.get::<_, String>(7)?)?,
                    resolved_at: resolved_at.map(parse_time).transpose()?,
                })
            })
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub fn load_session_settings_revision(
        &self,
        session_id: SessionId,
        settings_revision_id: u64,
    ) -> Result<Option<SessionSettingsRevisionProjection>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT session_id, settings_revision_id, connection_id, model_id, tool_mode, updated_at
                 FROM session_settings_revision_projection
                 WHERE session_id = ?1 AND settings_revision_id = ?2",
            )
            .map_err(storage_error)?;
        statement
            .query_row(
                params![session_id.to_string(), settings_revision_id as i64],
                |row| {
                    Ok(SessionSettingsRevisionProjection {
                        session_id: SessionId(
                            row.get::<_, String>(0)?
                                .parse()
                                .map_err(to_sql_conversion)?,
                        ),
                        settings_revision_id: row.get::<_, i64>(1)?.max(1) as u64,
                        connection_id: bt_core::ConnectionId::new(row.get::<_, String>(2)?),
                        model_id: row.get(3)?,
                        tool_mode: parse_session_tool_mode(row.get::<_, String>(4)?),
                        updated_at: parse_time(row.get::<_, String>(5)?)?,
                    })
                },
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn load_pending_steers(&self, session_id: SessionId) -> Result<Vec<SteerProjection>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT steer_event_id, session_id, branch_id, message, settings_revision_id, status, source_seq, enqueued_at, resolved_at
                 FROM steer_projection
                 WHERE session_id = ?1 AND status = 'pending'
                 ORDER BY source_seq ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![session_id.to_string()], |row| {
                let resolved_at: Option<String> = row.get(8)?;
                Ok(SteerProjection {
                    steer_event_id: row
                        .get::<_, String>(0)?
                        .parse()
                        .map_err(to_sql_conversion)?,
                    session_id: SessionId(
                        row.get::<_, String>(1)?
                            .parse()
                            .map_err(to_sql_conversion)?,
                    ),
                    branch_id: row
                        .get::<_, String>(2)?
                        .parse()
                        .map_err(to_sql_conversion)?,
                    message: row.get(3)?,
                    settings_revision_id: row.get::<_, i64>(4)?.max(1) as u64,
                    status: row.get(5)?,
                    source_seq: row.get(6)?,
                    enqueued_at: parse_time(row.get::<_, String>(7)?)?,
                    resolved_at: resolved_at.map(parse_time).transpose()?,
                })
            })
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub fn load_pending_steers_for_branch(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
    ) -> Result<Vec<SteerProjection>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT steer_event_id, session_id, branch_id, message, settings_revision_id, status, source_seq, enqueued_at, resolved_at
                 FROM steer_projection
                 WHERE session_id = ?1 AND branch_id = ?2 AND status = 'pending'
                 ORDER BY source_seq ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![session_id.to_string(), branch_id.to_string()],
                |row| {
                    let resolved_at: Option<String> = row.get(8)?;
                    Ok(SteerProjection {
                        steer_event_id: row
                            .get::<_, String>(0)?
                            .parse()
                            .map_err(to_sql_conversion)?,
                        session_id: SessionId(
                            row.get::<_, String>(1)?
                                .parse()
                                .map_err(to_sql_conversion)?,
                        ),
                        branch_id: row
                            .get::<_, String>(2)?
                            .parse()
                            .map_err(to_sql_conversion)?,
                        message: row.get(3)?,
                        settings_revision_id: row.get::<_, i64>(4)?.max(1) as u64,
                        status: row.get(5)?,
                        source_seq: row.get(6)?,
                        enqueued_at: parse_time(row.get::<_, String>(7)?)?,
                        resolved_at: resolved_at.map(parse_time).transpose()?,
                    })
                },
            )
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub fn load_session_settings_snapshot(
        &self,
        session_id: SessionId,
        settings_revision_id: u64,
    ) -> Result<Option<SessionSettingsSnapshot>> {
        Ok(self
            .load_session_settings_revision(session_id, settings_revision_id)?
            .map(|projection| SessionSettingsSnapshot {
                session_id: projection.session_id,
                settings_revision_id: projection.settings_revision_id,
                connection_id: projection.connection_id,
                model_id: projection.model_id,
                tool_mode: projection.tool_mode,
                updated_at: projection.updated_at,
            }))
    }

    pub fn load_reusable_approvals(&self) -> Result<Vec<ReusableApprovalProjection>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT event_json, seq_id
                 FROM events
                 WHERE event_kind = 'tool.approval.resolved'
                 ORDER BY seq_id ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], |row| {
                let raw: String = row.get(0)?;
                let seq_id: i64 = row.get(1)?;
                let mut event: EventEnvelope =
                    serde_json::from_str(&raw).map_err(to_sql_conversion)?;
                event.seq_id = Some(seq_id);
                Ok(event)
            })
            .map_err(storage_error)?;
        let mut approvals = Vec::new();
        for row in rows {
            let event = row.map_err(storage_error)?;
            let EventPayload::ToolApprovalResolved {
                request_fingerprint,
                resolution,
                decision,
                ..
            } = event.payload
            else {
                continue;
            };
            let fingerprint = resolution
                .map(|resolution| resolution.request_fingerprint)
                .or(request_fingerprint);
            let scope = match &decision {
                bt_core::ApprovalDecision::Approved { scope, .. }
                | bt_core::ApprovalDecision::Denied { scope, .. } => *scope,
            };
            if !matches!(scope, ApprovalScope::Session | ApprovalScope::Always) {
                continue;
            }
            if let Some(request_fingerprint) = fingerprint {
                approvals.push(ReusableApprovalProjection {
                    session_id: event.session_id,
                    request_fingerprint,
                    decision,
                });
            }
        }
        Ok(approvals)
    }

    pub fn load_unfinished_unbound_tool_operations(
        &self,
    ) -> Result<Vec<ToolOperationRecoveryRecord>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT event_json, seq_id
                 FROM events
                 WHERE event_kind IN (
                    'tool.operation.recorded',
                    'tool.call.requested',
                    'tool.execution.finished'
                 )
                 ORDER BY seq_id ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], |row| {
                let raw: String = row.get(0)?;
                let seq_id: i64 = row.get(1)?;
                let mut event: EventEnvelope =
                    serde_json::from_str(&raw).map_err(to_sql_conversion)?;
                event.seq_id = Some(seq_id);
                Ok(event)
            })
            .map_err(storage_error)?;

        let mut operations: HashMap<(SessionId, ToolCallId), ToolOperationRecoveryRecord> =
            HashMap::new();
        let mut contexts: HashMap<(SessionId, ToolCallId), ToolOperationContext> = HashMap::new();
        for row in rows {
            let event = row.map_err(storage_error)?;
            if event.turn_id.is_some() {
                continue;
            }
            let key_for = |call_id: &ToolCallId| (event.session_id, call_id.clone());
            match event.payload {
                EventPayload::ToolOperationRecorded {
                    call_id, operation, ..
                } => {
                    contexts.insert(key_for(&call_id), operation);
                }
                EventPayload::ToolCallRequested {
                    call_id, tool_name, ..
                } => {
                    let key = key_for(&call_id);
                    operations.insert(
                        key.clone(),
                        ToolOperationRecoveryRecord {
                            session_id: event.session_id,
                            branch_id: event.branch_id,
                            call_id,
                            tool_name,
                            operation: contexts.get(&key).cloned().unwrap_or_default(),
                            requested_seq_id: event.seq_id.expect("stored event has seq id"),
                        },
                    );
                }
                EventPayload::ToolExecutionFinished { call_id, .. } => {
                    operations.remove(&key_for(&call_id));
                }
                _ => {}
            }
        }
        let mut unfinished = operations.into_values().collect::<Vec<_>>();
        unfinished.sort_by_key(|record| record.requested_seq_id);
        Ok(unfinished)
    }

    pub fn load_tool_runs(&self, session_id: SessionId) -> Result<Vec<ToolRunProjection>> {
        let mut statement = self.connection.prepare(
            "SELECT call_id, session_id, tool_name, status, arguments_json, result_json, updated_at
             FROM tool_run_projection
             WHERE session_id = ?1
             ORDER BY updated_at ASC",
        ).map_err(storage_error)?;
        let rows = statement
            .query_map(params![session_id.to_string()], |row| {
                Ok(ToolRunProjection {
                    call_id: ToolCallId::new(row.get::<_, String>(0)?),
                    session_id: SessionId(
                        row.get::<_, String>(1)?
                            .parse()
                            .map_err(to_sql_conversion)?,
                    ),
                    tool_name: row.get(2)?,
                    status: row.get(3)?,
                    arguments: row
                        .get::<_, Option<String>>(4)?
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(to_sql_conversion)?,
                    result: row
                        .get::<_, Option<String>>(5)?
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(to_sql_conversion)?,
                    updated_at: parse_time(row.get::<_, String>(6)?)?,
                })
            })
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub fn load_branch_plan(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
    ) -> Result<Option<PlanProjection>> {
        self.connection
            .query_row(
                "SELECT session_id, branch_id, items_json, updated_at
                 FROM plan_projection
                 WHERE session_id = ?1 AND branch_id = ?2",
                params![session_id.to_string(), branch_id.to_string()],
                |row| {
                    Ok(PlanProjection {
                        session_id: SessionId(
                            row.get::<_, String>(0)?
                                .parse()
                                .map_err(to_sql_conversion)?,
                        ),
                        branch_id: BranchId(
                            row.get::<_, String>(1)?
                                .parse()
                                .map_err(to_sql_conversion)?,
                        ),
                        items: serde_json::from_str(&row.get::<_, String>(2)?)
                            .map_err(to_sql_conversion)?,
                        updated_at: parse_time(row.get::<_, String>(3)?)?,
                    })
                },
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn load_branch_heads(&self, session_id: SessionId) -> Result<Vec<BranchHead>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT branch_id, session_id, head_event_seq, updated_at
             FROM branch_heads
             WHERE session_id = ?1
             ORDER BY updated_at ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![session_id.to_string()], |row| {
                Ok(BranchHead {
                    branch_id: BranchId(
                        row.get::<_, String>(0)?
                            .parse()
                            .map_err(to_sql_conversion)?,
                    ),
                    session_id: SessionId(
                        row.get::<_, String>(1)?
                            .parse()
                            .map_err(to_sql_conversion)?,
                    ),
                    head_seq_id: row.get(2)?,
                    updated_at: parse_time(row.get::<_, String>(3)?)?,
                })
            })
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub fn load_cost_summary(
        &self,
        session_id: SessionId,
    ) -> Result<Option<CostSummaryProjection>> {
        self.connection
            .query_row(
                "SELECT session_id, prompt_tokens, completion_tokens, total_tokens, total_cost_usd, unpriced_completion_count, updated_at
                 FROM cost_summary_projection
                 WHERE session_id = ?1",
                params![session_id.to_string()],
                |row| {
                    Ok(CostSummaryProjection {
                        session_id: SessionId(row.get::<_, String>(0)?.parse().map_err(to_sql_conversion)?),
                        prompt_tokens: row.get(1)?,
                        completion_tokens: row.get(2)?,
                        total_tokens: row.get(3)?,
                        total_cost_usd: row.get(4)?,
                        unpriced_completion_count: row.get(5)?,
                        updated_at: parse_time(row.get::<_, String>(6)?)?,
                    })
                },
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn load_session_budget(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SessionBudgetProjection>> {
        self.connection
            .query_row(
                "SELECT session_id, max_wall_clock_seconds, max_tokens, max_turns, max_cost_usd, tokens_used, turns_used, elapsed_seconds, cost_used_usd, updated_at
                 FROM session_budget_projection
                 WHERE session_id = ?1",
                params![session_id.to_string()],
                |row| {
                    Ok(SessionBudgetProjection {
                        session_id: SessionId(
                            row.get::<_, String>(0)?.parse().map_err(to_sql_conversion)?,
                        ),
                        budget: BudgetConfig {
                            max_wall_clock_seconds: row
                                .get::<_, Option<i64>>(1)?
                                .map(|value| u64::try_from(value).map_err(to_sql_conversion))
                                .transpose()?,
                            max_tokens: row
                                .get::<_, Option<i64>>(2)?
                                .map(|value| u64::try_from(value).map_err(to_sql_conversion))
                                .transpose()?,
                            max_turns: row
                                .get::<_, Option<i64>>(3)?
                                .map(|value| u32::try_from(value).map_err(to_sql_conversion))
                                .transpose()?,
                            max_cost_usd: row.get(4)?,
                        },
                        tokens_used: row.get::<_, i64>(5)?.max(0) as u64,
                        turns_used: row.get::<_, i64>(6)?.max(0) as u32,
                        elapsed_seconds: row.get::<_, i64>(7)?.max(0) as u64,
                        cost_used_usd: row.get(8)?,
                        updated_at: parse_time(row.get::<_, String>(9)?)?,
                    })
                },
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn reproject_completion_costs<R>(
        &mut self,
        session_id: SessionId,
        mut reprojector: R,
    ) -> Result<ReprojectionReport>
    where
        R: CompletionCostReprojector,
    {
        let tx = self.connection.transaction().map_err(storage_error)?;
        let events = load_session_event_log_from_connection(&tx, session_id)?;
        tx.execute(
            "DELETE FROM cost_summary_projection WHERE session_id = ?1",
            params![session_id.to_string()],
        )
        .map_err(storage_error)?;

        let mut report = ReprojectionReport::default();
        for stored in events {
            report.events_scanned += 1;
            let EventPayload::CompletionFinished {
                provider,
                model,
                usage,
                cost,
                ..
            } = &stored.event.payload
            else {
                continue;
            };

            report.completion_events_scanned += 1;
            let projected_cost =
                reprojector.reproject_completion_cost(provider, model, usage, cost.as_ref())?;
            if projected_cost.as_ref() != cost.as_ref() {
                report.completion_costs_changed += 1;
            }
            Self::refresh_cost_summary_projection(
                &tx,
                stored.event.session_id,
                stored.event.occurred_at,
                usage,
                projected_cost.as_ref(),
            )?;
        }

        tx.commit().map_err(storage_error)?;
        Ok(report)
    }

    pub fn reproject_session_budget(&mut self, session_id: SessionId) -> Result<()> {
        let tx = self.connection.transaction().map_err(storage_error)?;
        let events = load_session_event_log_from_connection(&tx, session_id)?;
        tx.execute(
            "DELETE FROM session_budget_projection WHERE session_id = ?1",
            params![session_id.to_string()],
        )
        .map_err(storage_error)?;

        for stored in events {
            match &stored.event.payload {
                EventPayload::BudgetConfigured { budget } => {
                    Self::refresh_budget_projection_from_config(
                        &tx,
                        stored.event.session_id,
                        stored.event.occurred_at,
                        budget,
                    )?
                }
                EventPayload::BudgetCheckpoint {
                    tokens_used,
                    max_tokens,
                    turns_used,
                    max_turns,
                    elapsed_seconds,
                    max_wall_clock_seconds,
                    cost_used_usd,
                    max_cost_usd,
                } => Self::refresh_budget_projection_from_checkpoint(
                    &tx,
                    stored.event.session_id,
                    stored.event.occurred_at,
                    &BudgetConfig {
                        max_wall_clock_seconds: *max_wall_clock_seconds,
                        max_tokens: *max_tokens,
                        max_turns: *max_turns,
                        max_cost_usd: *max_cost_usd,
                    },
                    *tokens_used,
                    *turns_used,
                    *elapsed_seconds,
                    *cost_used_usd,
                )?,
                _ => {}
            }
        }

        tx.commit().map_err(storage_error)?;
        Ok(())
    }

    pub fn load_session_inspection_metrics(
        &self,
        session_id: SessionId,
    ) -> Result<SessionInspectionMetrics> {
        let message_count = count_query(
            &self.connection,
            "SELECT COUNT(*) FROM message_projection WHERE session_id = ?1",
            session_id,
        )?;
        let tool_call_count = count_query(
            &self.connection,
            "SELECT COUNT(*) FROM tool_run_projection WHERE session_id = ?1",
            session_id,
        )?;
        let approval_count = count_query(
            &self.connection,
            "SELECT COUNT(*) FROM approval_projection WHERE session_id = ?1",
            session_id,
        )?;
        let pending_approval_count = count_query(
            &self.connection,
            "SELECT COUNT(*) FROM approval_projection WHERE session_id = ?1 AND status = 'pending'",
            session_id,
        )?;
        let raw_chunk_count = count_query(
            &self.connection,
            "SELECT COUNT(*) FROM raw_chunks WHERE session_id = ?1",
            session_id,
        )?;
        let active_turn_count = count_query(
            &self.connection,
            "SELECT COUNT(*) FROM active_turn_projection WHERE session_id = ?1",
            session_id,
        )?;
        let turn_count = count_query(
            &self.connection,
            "SELECT COUNT(*) FROM events WHERE session_id = ?1 AND event_kind = 'turn.started'",
            session_id,
        )?;
        let last_seq_id = optional_i64_query(
            &self.connection,
            "SELECT MAX(seq_id) FROM events WHERE session_id = ?1",
            session_id,
        )?;
        Ok(SessionInspectionMetrics {
            last_seq_id,
            turn_count,
            message_count,
            tool_call_count,
            approval_count,
            pending_approval_count,
            raw_chunk_count,
            active_turn_count,
        })
    }
}

#[cfg(test)]
mod tests;
