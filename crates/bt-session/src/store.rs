use crate::{CompletionCostReprojector, ReprojectionReport, StoredSessionEvent, apply_migrations};
use bt_core::{
    ApprovalScope, BelltowerError, BranchHead, BranchId, BranchRecord, BudgetConfig, ConnectionId,
    EventEnvelope, EventPayload, MAX_RELATED_SESSION_DEPTH, MAX_RELATED_SESSION_DESCENDANTS,
    Message, RelatedSessionDeliveryMode, RelatedSessionMessage, RelatedSessionMessageDirection,
    RelatedSessionMessageId, RelatedSessionMessageReceipt, RelatedSessionMessageRecord,
    RelatedSessionMessageStatus, Result, Role, SessionId, SessionRecord, SessionSettingsSnapshot,
    SessionToolMode, SpanKind, ToolCallId, ToolOperationContext, TurnId, TurnInspection,
    TurnStartSource,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use time::OffsetDateTime;

const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

fn parse_related_message_status(value: &str) -> Result<RelatedSessionMessageStatus> {
    match value {
        "delivered" => Ok(RelatedSessionMessageStatus::Delivered),
        "pending" => Ok(RelatedSessionMessageStatus::Pending),
        "claimed" => Ok(RelatedSessionMessageStatus::Claimed),
        "dropped" => Ok(RelatedSessionMessageStatus::Dropped),
        other => Err(BelltowerError::Storage(format!(
            "unknown related-session message status `{other}`"
        ))),
    }
}

fn parse_related_message_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<RelatedSessionMessageRecord> {
    let message_json: String = row.get(0)?;
    let direction: String = row.get(1)?;
    let status: String = row.get(2)?;
    let event_id: String = row.get(3)?;
    let counterpart_event_id: String = row.get(4)?;
    let resulting_turn_id: Option<String> = row.get(6)?;
    let resolved_at: Option<String> = row.get(7)?;
    Ok(RelatedSessionMessageRecord {
        message: serde_json::from_str(&message_json).map_err(to_sql_conversion)?,
        direction: match direction.as_str() {
            "sent" => RelatedSessionMessageDirection::Sent,
            "received" => RelatedSessionMessageDirection::Received,
            other => {
                return Err(to_sql_conversion(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unknown related-session message direction `{other}`"),
                )));
            }
        },
        status: parse_related_message_status(&status).map_err(to_sql_conversion)?,
        event_id: event_id.parse().map_err(to_sql_conversion)?,
        counterpart_event_id: counterpart_event_id.parse().map_err(to_sql_conversion)?,
        seq_id: row.get(5)?,
        resulting_turn_id: resulting_turn_id
            .map(|turn_id| turn_id.parse().map_err(to_sql_conversion))
            .transpose()?,
        resolved_at: resolved_at.map(parse_time).transpose()?,
    })
}

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
    validate_queued_continuation_events, validate_resumed_continuation_events,
    validate_steer_continuation_events, validate_turn_admission_events,
};
mod types;
pub use types::{
    ApprovalProjection, ChunkPage, CommittedBudgetCheckpoint, CommittedBudgetConfiguration,
    CommittedSessionSettingsUpdate, ContextManifestProjection, ContextMessageRecord,
    ContinuationClaim, CostSummaryProjection, PlanProjection, QueuedMessageProjection,
    RawChunkInsert, RawChunkRecord, RecordedOperatorCommandRecord, RelatedMessageProjection,
    ResumedContinuationKind, ResumedTurnRecoveryRecord, ReusableApprovalProjection,
    SequencedMessageRecord, SessionBudgetProjection, SessionControlProjection,
    SessionInspectionMetrics, SessionSettingsRevisionProjection, SessionTurnAdmission,
    SteerProjection, ToolOperationRecoveryRecord, ToolRunProjection, TranscriptPage,
    TurnRecoveryRecord,
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

    pub fn commit_child_session_spawn(
        &mut self,
        child_session: &SessionRecord,
        child_branch: &BranchRecord,
        spawn_requested: &EventEnvelope,
        child_started: &EventEnvelope,
        child_handoff: &EventEnvelope,
        spawn_completed: &EventEnvelope,
    ) -> Result<Vec<i64>> {
        self.commit_child_session_spawn_transition(
            child_session,
            child_branch,
            spawn_requested,
            child_started,
            child_handoff,
            spawn_completed,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn commit_child_session_spawn_with_initial_message(
        &mut self,
        child_session: &SessionRecord,
        child_branch: &BranchRecord,
        spawn_requested: &EventEnvelope,
        child_started: &EventEnvelope,
        child_handoff: &EventEnvelope,
        spawn_completed: &EventEnvelope,
        sent: &EventEnvelope,
        received: &EventEnvelope,
    ) -> Result<Vec<i64>> {
        self.commit_child_session_spawn_transition(
            child_session,
            child_branch,
            spawn_requested,
            child_started,
            child_handoff,
            spawn_completed,
            Some((sent, received)),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_child_session_spawn_transition(
        &mut self,
        child_session: &SessionRecord,
        child_branch: &BranchRecord,
        spawn_requested: &EventEnvelope,
        child_started: &EventEnvelope,
        child_handoff: &EventEnvelope,
        spawn_completed: &EventEnvelope,
        initial_message: Option<(&EventEnvelope, &EventEnvelope)>,
    ) -> Result<Vec<i64>> {
        let objective = child_session.objective.as_deref().ok_or_else(|| {
            BelltowerError::InvalidState("child session spawn requires an objective".to_owned())
        })?;
        let parent_session_id = child_session.parent_session_id.ok_or_else(|| {
            BelltowerError::InvalidState("child session spawn requires a parent session".to_owned())
        })?;
        let parent_branch_id = child_session.parent_branch_id.ok_or_else(|| {
            BelltowerError::InvalidState("child session spawn requires a parent branch".to_owned())
        })?;
        let child_connection_id = child_session.connection_id.to_string();
        let valid_scope = child_branch.session_id == child_session.session_id
            && spawn_requested.session_id == parent_session_id
            && spawn_requested.branch_id == parent_branch_id
            && spawn_requested.turn_id == child_session.parent_turn_id
            && child_started.session_id == child_session.session_id
            && child_started.branch_id == child_branch.branch_id
            && child_started.turn_id.is_none()
            && child_handoff.session_id == child_session.session_id
            && child_handoff.branch_id == child_branch.branch_id
            && child_handoff.turn_id.is_none()
            && spawn_completed.session_id == parent_session_id
            && spawn_completed.branch_id == parent_branch_id
            && spawn_completed.turn_id == child_session.parent_turn_id;
        let valid_payloads = matches!(
            &spawn_requested.payload,
            EventPayload::SessionSpawnRequested {
                child_session_id,
                objective: event_objective,
                connection_id,
                model_id,
            } if *child_session_id == child_session.session_id
                && event_objective == objective
                && connection_id == &child_connection_id
                && model_id == &child_session.model_id
        ) && matches!(
            &child_started.payload,
            EventPayload::SessionStarted {
                project_root,
                connection_id,
            } if project_root == child_session.project_root.as_str()
                && connection_id == &child_connection_id
        ) && matches!(
            &child_handoff.payload,
            EventPayload::SessionHandoffRecorded {
                parent_session_id: event_parent_session_id,
                parent_branch_id: event_parent_branch_id,
                parent_turn_id,
                objective: event_objective,
                summary,
            } if *event_parent_session_id == parent_session_id
                && *event_parent_branch_id == parent_branch_id
                && *parent_turn_id == child_session.parent_turn_id
                && event_objective == objective
                && summary == objective
        ) && matches!(
            &spawn_completed.payload,
            EventPayload::SessionSpawned {
                child_session_id,
                child_branch_id,
                objective: event_objective,
            } if *child_session_id == child_session.session_id
                && *child_branch_id == child_branch.branch_id
                && event_objective == objective
        );
        if !valid_scope || !valid_payloads {
            return Err(BelltowerError::InvalidState(
                "child session spawn events do not match the canonical parent/child records"
                    .to_owned(),
            ));
        }

        if let Some((sent, received)) = initial_message {
            let message = Self::validate_related_session_message_envelopes(sent, received)?;
            if message.source_session_id != parent_session_id
                || message.source_branch_id != parent_branch_id
                || message.destination_session_id != child_session.session_id
                || message.destination_branch_id != child_branch.branch_id
                || !matches!(message.delivery_mode, RelatedSessionDeliveryMode::Wake)
            {
                return Err(BelltowerError::InvalidState(
                    "initial child message does not match the canonical spawn".to_owned(),
                ));
            }
        }

        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        Self::ensure_child_spawn_bounds_in_tx(&tx, parent_session_id)?;
        Self::insert_session_in_tx(&tx, child_session)?;
        Self::insert_session_settings_revision_in_tx(&tx, child_session)?;
        Self::insert_branch_in_tx(&tx, child_branch)?;
        let mut events = vec![
            spawn_requested.clone(),
            child_started.clone(),
            child_handoff.clone(),
            spawn_completed.clone(),
        ];
        if let Some((sent, received)) = initial_message {
            let message = Self::validate_related_session_message_envelopes(sent, received)?;
            Self::ensure_related_session_message_constraints_in_tx(&tx, message)?;
            events.push(sent.clone());
            events.push(received.clone());
        }
        let seq_ids = Self::append_events_in_tx(&tx, &events)?;
        tx.commit().map_err(storage_error)?;
        Ok(seq_ids)
    }

    pub fn commit_related_session_message(
        &mut self,
        sent: &EventEnvelope,
        received: &EventEnvelope,
    ) -> Result<RelatedSessionMessageReceipt> {
        let sent_message = Self::validate_related_session_message_envelopes(sent, received)?;

        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        if let Some(receipt) = Self::load_related_message_receipt_in_tx(
            &tx,
            sent_message.source_session_id,
            sent_message.destination_session_id,
            sent_message.message_id,
            Some(sent_message),
        )? {
            tx.commit().map_err(storage_error)?;
            return Ok(receipt);
        }

        Self::ensure_related_session_message_constraints_in_tx(&tx, sent_message)?;

        let sent_seq_id = Self::append_event_in_tx(&tx, sent)?;
        let received_seq_id = Self::append_event_in_tx(&tx, received)?;
        tx.commit().map_err(storage_error)?;
        Ok(RelatedSessionMessageReceipt {
            message_id: sent_message.message_id,
            sent_event_id: sent.event_id,
            sent_seq_id,
            received_event_id: received.event_id,
            received_seq_id,
            status: if matches!(sent_message.delivery_mode, RelatedSessionDeliveryMode::Wake) {
                RelatedSessionMessageStatus::Pending
            } else {
                RelatedSessionMessageStatus::Delivered
            },
        })
    }

    fn validate_related_session_message_envelopes<'a>(
        sent: &'a EventEnvelope,
        received: &EventEnvelope,
    ) -> Result<&'a RelatedSessionMessage> {
        let (
            EventPayload::RelatedSessionMessageRecorded {
                direction: RelatedSessionMessageDirection::Sent,
                counterpart_event_id: sent_counterpart,
                message: sent_message,
            },
            EventPayload::RelatedSessionMessageRecorded {
                direction: RelatedSessionMessageDirection::Received,
                counterpart_event_id: received_counterpart,
                message: received_message,
            },
        ) = (&sent.payload, &received.payload)
        else {
            return Err(BelltowerError::InvalidState(
                "related-session delivery requires one sent and one received event".to_owned(),
            ));
        };
        if sent_message != received_message
            || sent.event_id != *received_counterpart
            || received.event_id != *sent_counterpart
            || sent.session_id != sent_message.source_session_id
            || sent.branch_id != sent_message.source_branch_id
            || sent.turn_id != sent_message.caused_by_turn_id
            || received.session_id != sent_message.destination_session_id
            || received.branch_id != sent_message.destination_branch_id
            || received.turn_id.is_some()
            || sent_message.text.trim().is_empty()
        {
            return Err(BelltowerError::InvalidState(
                "related-session delivery envelopes do not match the canonical message".to_owned(),
            ));
        }
        Ok(sent_message)
    }

    fn ensure_related_session_message_constraints_in_tx(
        tx: &Transaction<'_>,
        message: &RelatedSessionMessage,
    ) -> Result<()> {
        let source_parent = Self::parent_session_id_in_tx(tx, message.source_session_id)?;
        let destination_parent = Self::parent_session_id_in_tx(tx, message.destination_session_id)?;
        if source_parent != Some(message.destination_session_id)
            && destination_parent != Some(message.source_session_id)
        {
            return Err(BelltowerError::Protocol(
                "related-session messages are limited to direct parent-child sessions".to_owned(),
            ));
        }
        Self::ensure_branch_owner_in_tx(tx, message.source_session_id, message.source_branch_id)?;
        Self::ensure_branch_owner_in_tx(
            tx,
            message.destination_session_id,
            message.destination_branch_id,
        )?;
        if let Some(turn_id) = message.caused_by_turn_id {
            let owns_active_turn: i64 = tx
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM active_turn_projection
                        WHERE session_id = ?1 AND branch_id = ?2 AND turn_id = ?3
                    )",
                    params![
                        message.source_session_id.to_string(),
                        message.source_branch_id.to_string(),
                        turn_id.to_string(),
                    ],
                    |row| row.get(0),
                )
                .map_err(storage_error)?;
            if owns_active_turn == 0 {
                return Err(BelltowerError::Protocol(
                    "related-session message causal turn is not the sender's active turn"
                        .to_owned(),
                ));
            }
        }
        if let Some(reply_id) = message.in_reply_to {
            let reply_exists: i64 = tx
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM related_session_message_projection
                        WHERE message_id = ?1
                          AND ((source_session_id = ?2 AND destination_session_id = ?3)
                            OR (source_session_id = ?3 AND destination_session_id = ?2))
                    )",
                    params![
                        reply_id.to_string(),
                        message.source_session_id.to_string(),
                        message.destination_session_id.to_string(),
                    ],
                    |row| row.get(0),
                )
                .map_err(storage_error)?;
            if reply_exists == 0 {
                return Err(BelltowerError::Protocol(
                    "related-session reply target was not found between these sessions".to_owned(),
                ));
            }
        }
        Ok(())
    }

    fn ensure_child_spawn_bounds_in_tx(
        tx: &Transaction<'_>,
        parent_session_id: SessionId,
    ) -> Result<()> {
        let (root_id, ancestor_count): (String, i64) = tx
            .query_row(
                "WITH RECURSIVE ancestors(session_id, parent_session_id) AS (
                    SELECT session_id, parent_session_id FROM sessions WHERE session_id = ?1
                    UNION
                    SELECT sessions.session_id, sessions.parent_session_id
                    FROM sessions JOIN ancestors ON sessions.session_id = ancestors.parent_session_id
                 )
                 SELECT session_id, (SELECT COUNT(*) FROM ancestors)
                 FROM ancestors WHERE parent_session_id IS NULL LIMIT 1",
                params![parent_session_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(storage_error)?
            .ok_or_else(|| {
                BelltowerError::InvalidState(
                    "session lineage has no reachable root session".to_owned(),
                )
            })?;
        let depth = ancestor_count.saturating_sub(1) as usize;
        if depth >= MAX_RELATED_SESSION_DEPTH {
            return Err(BelltowerError::Protocol(format!(
                "subagent depth limit of {MAX_RELATED_SESSION_DEPTH} reached"
            )));
        }
        let lineage_size: i64 = tx
            .query_row(
                "WITH RECURSIVE lineage(session_id) AS (
                    SELECT ?1
                    UNION
                    SELECT sessions.session_id
                    FROM sessions JOIN lineage ON sessions.parent_session_id = lineage.session_id
                 )
                 SELECT COUNT(*) FROM lineage",
                params![root_id],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        let descendants = lineage_size.saturating_sub(1) as usize;
        if descendants >= MAX_RELATED_SESSION_DESCENDANTS {
            return Err(BelltowerError::Protocol(format!(
                "subagent lineage limit of {MAX_RELATED_SESSION_DESCENDANTS} reached"
            )));
        }
        Ok(())
    }

    fn parent_session_id_in_tx(
        tx: &Transaction<'_>,
        session_id: SessionId,
    ) -> Result<Option<SessionId>> {
        let raw = tx
            .query_row(
                "SELECT parent_session_id FROM sessions WHERE session_id = ?1",
                params![session_id.to_string()],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(storage_error)?
            .ok_or_else(|| BelltowerError::NotFound(format!("session `{session_id}`")))?;
        raw.map(|value| {
            value.parse().map_err(|error| {
                BelltowerError::Storage(format!("invalid parent session id `{value}`: {error}"))
            })
        })
        .transpose()
    }

    fn ensure_branch_owner_in_tx(
        tx: &Transaction<'_>,
        session_id: SessionId,
        branch_id: BranchId,
    ) -> Result<()> {
        let owns_branch: i64 = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM branches WHERE branch_id = ?1 AND session_id = ?2)",
                params![branch_id.to_string(), session_id.to_string()],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        if owns_branch == 0 {
            return Err(BelltowerError::Protocol(format!(
                "branch `{branch_id}` does not belong to session `{session_id}`"
            )));
        }
        Ok(())
    }

    fn load_related_message_receipt_in_tx(
        tx: &Transaction<'_>,
        source_session_id: SessionId,
        destination_session_id: SessionId,
        message_id: RelatedSessionMessageId,
        expected: Option<&RelatedSessionMessage>,
    ) -> Result<Option<RelatedSessionMessageReceipt>> {
        let sent = tx
            .query_row(
                "SELECT event_id, counterpart_event_id, message_json, source_seq
                 FROM related_session_message_projection
                 WHERE session_id = ?1 AND message_id = ?2 AND direction = 'sent'",
                params![source_session_id.to_string(), message_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_error)?;
        let Some((sent_event_id, received_event_id, message_json, sent_seq_id)) = sent else {
            return Ok(None);
        };
        let stored_message: RelatedSessionMessage = serde_json::from_str(&message_json)?;
        if expected.is_some_and(|expected| expected != &stored_message) {
            return Err(BelltowerError::Protocol(format!(
                "related-session message id `{message_id}` was reused with different content"
            )));
        }
        let (received_seq_id, status) = tx
            .query_row(
                "SELECT source_seq, status FROM related_session_message_projection
                 WHERE session_id = ?1 AND message_id = ?2 AND direction = 'received'",
                params![destination_session_id.to_string(), message_id.to_string()],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(storage_error)?;
        Ok(Some(RelatedSessionMessageReceipt {
            message_id,
            sent_event_id: sent_event_id.parse().map_err(|error| {
                BelltowerError::Storage(format!("invalid related message event id: {error}"))
            })?,
            sent_seq_id,
            received_event_id: received_event_id.parse().map_err(|error| {
                BelltowerError::Storage(format!("invalid related message event id: {error}"))
            })?,
            received_seq_id,
            status: parse_related_message_status(&status)?,
        }))
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

    pub fn commit_active_turn_events(
        &mut self,
        session_id: SessionId,
        branch_id: BranchId,
        turn_id: TurnId,
        events: &[EventEnvelope],
    ) -> Result<Vec<i64>> {
        Self::validate_active_turn_events(session_id, branch_id, turn_id, events)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        Self::ensure_active_turn_owner_in_tx(&tx, session_id, branch_id, turn_id)?;
        let seq_ids = Self::append_events_in_tx(&tx, events)?;
        tx.commit().map_err(storage_error)?;
        Ok(seq_ids)
    }

    pub fn commit_turn_terminal_transition(
        &mut self,
        session_id: SessionId,
        branch_id: BranchId,
        turn_id: TurnId,
        terminal_events: &[EventEnvelope],
    ) -> Result<Vec<i64>> {
        Self::validate_turn_terminal_events(session_id, branch_id, turn_id, terminal_events)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        Self::ensure_active_turn_owner_in_tx(&tx, session_id, branch_id, turn_id)?;
        let seq_ids = Self::append_events_in_tx(&tx, terminal_events)?;
        tx.commit().map_err(storage_error)?;
        Ok(seq_ids)
    }

    pub fn commit_budget_checkpoint(
        &mut self,
        checkpoint: &EventEnvelope,
        cancellation: &EventEnvelope,
    ) -> Result<CommittedBudgetCheckpoint> {
        self.commit_budget_checkpoint_transition(checkpoint, cancellation, &[], false)
    }

    pub fn commit_budget_terminal_transition(
        &mut self,
        checkpoint: &EventEnvelope,
        cancellation: &EventEnvelope,
        terminal_events: &[EventEnvelope],
    ) -> Result<CommittedBudgetCheckpoint> {
        self.commit_budget_checkpoint_transition(checkpoint, cancellation, terminal_events, true)
    }

    fn commit_budget_checkpoint_transition(
        &mut self,
        checkpoint: &EventEnvelope,
        cancellation: &EventEnvelope,
        terminal_events: &[EventEnvelope],
        require_terminal_finish: bool,
    ) -> Result<CommittedBudgetCheckpoint> {
        if checkpoint.session_id != cancellation.session_id
            || checkpoint.branch_id != cancellation.branch_id
            || !matches!(&checkpoint.payload, EventPayload::BudgetCheckpoint { .. })
            || !matches!(&cancellation.payload, EventPayload::SessionCancelled { .. })
        {
            return Err(BelltowerError::InvalidState(
                "budget checkpoint transition contains inconsistent events".to_owned(),
            ));
        }
        if terminal_events.iter().any(|event| {
            event.session_id != checkpoint.session_id
                || event.branch_id != checkpoint.branch_id
                || event.turn_id != checkpoint.turn_id
        }) {
            return Err(BelltowerError::InvalidState(
                "budget terminal transition contains events outside the checkpoint turn".to_owned(),
            ));
        }
        if require_terminal_finish
            && !matches!(
                terminal_events.last().map(|event| &event.payload),
                Some(EventPayload::TurnFinished { turn_id, .. })
                    if Some(*turn_id) == checkpoint.turn_id
            )
        {
            return Err(BelltowerError::InvalidState(
                "budget terminal transition must end with turn.finished".to_owned(),
            ));
        }

        let turn_id = checkpoint.turn_id.ok_or_else(|| {
            BelltowerError::InvalidState(
                "budget checkpoint transition is missing a turn id".to_owned(),
            )
        })?;
        if require_terminal_finish {
            Self::validate_turn_terminal_events(
                checkpoint.session_id,
                checkpoint.branch_id,
                turn_id,
                terminal_events,
            )?;
        }

        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        Self::ensure_active_turn_owner_in_tx(
            &tx,
            checkpoint.session_id,
            checkpoint.branch_id,
            turn_id,
        )?;
        let checkpoint_seq_id = Self::append_event_in_tx(&tx, checkpoint)?;
        let exhausted = Self::session_budget_exhausted_in_tx(&tx, checkpoint.session_id)?;
        let cancellation_seq_id = exhausted
            .then(|| Self::append_event_in_tx(&tx, cancellation))
            .transpose()?;
        let terminal_seq_ids = Self::append_events_in_tx(&tx, terminal_events)?;
        tx.commit().map_err(storage_error)?;
        Ok(CommittedBudgetCheckpoint {
            checkpoint_seq_id,
            cancellation_seq_id,
            terminal_seq_ids,
            exhausted,
        })
    }

    pub fn commit_budget_configuration(
        &mut self,
        configuration: &EventEnvelope,
        cancellation: &EventEnvelope,
    ) -> Result<CommittedBudgetConfiguration> {
        if configuration.session_id != cancellation.session_id
            || configuration.branch_id != cancellation.branch_id
            || !matches!(
                &configuration.payload,
                EventPayload::BudgetConfigured { .. }
            )
            || !matches!(&cancellation.payload, EventPayload::SessionCancelled { .. })
        {
            return Err(BelltowerError::InvalidState(
                "budget configuration transition contains inconsistent events".to_owned(),
            ));
        }

        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        if Self::active_turn_exists_in_tx(&tx, configuration.session_id)? {
            return Err(BelltowerError::InvalidState(
                "session budget can only be configured while the session is idle".to_owned(),
            ));
        }
        let configuration_seq_id = Self::append_event_in_tx(&tx, configuration)?;
        let exhausted = Self::session_budget_exhausted_in_tx(&tx, configuration.session_id)?;
        // A prior operator cancellation is not evidence that this budget transition
        // exhausted the session. Preserve the distinct budget stop reason durably.
        let cancellation_seq_id = exhausted
            .then(|| Self::append_event_in_tx(&tx, cancellation))
            .transpose()?;
        tx.commit().map_err(storage_error)?;
        Ok(CommittedBudgetConfiguration {
            configuration_seq_id,
            cancellation_seq_id,
            exhausted,
        })
    }

    pub fn commit_session_settings_update(
        &mut self,
        session_id: SessionId,
        branch_id: BranchId,
        connection_id: Option<ConnectionId>,
        model_id: Option<Option<String>>,
        tool_mode: Option<SessionToolMode>,
    ) -> Result<CommittedSessionSettingsUpdate> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let current = tx
            .query_row(
                "SELECT connection_id, model_id, tool_mode, settings_revision_id
                 FROM sessions WHERE session_id = ?1",
                params![session_id.to_string()],
                |row| {
                    Ok((
                        ConnectionId::new(row.get::<_, String>(0)?),
                        row.get::<_, Option<String>>(1)?,
                        parse_session_tool_mode(row.get::<_, String>(2)?),
                        row.get::<_, i64>(3)?.max(1) as u64,
                    ))
                },
            )
            .optional()
            .map_err(storage_error)?
            .ok_or_else(|| BelltowerError::InvalidState("session not found".to_owned()))?;
        let settings_revision_id = current.3.checked_add(1).ok_or_else(|| {
            BelltowerError::InvalidState("session settings revision exhausted".to_owned())
        })?;
        i64::try_from(settings_revision_id).map_err(|_| {
            BelltowerError::InvalidState("session settings revision exhausted".to_owned())
        })?;
        let connection_id = connection_id.unwrap_or(current.0);
        let model_id = model_id.unwrap_or(current.1);
        let tool_mode = tool_mode.unwrap_or(current.2);
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Session,
            EventPayload::SessionSettingsUpdated {
                settings_revision_id,
                connection_id: connection_id.to_string(),
                model_id: model_id.clone(),
                tool_mode: session_tool_mode_to_str(tool_mode).to_owned(),
            },
        );
        let seq_id = Self::append_event_in_tx(&tx, &event)?;
        let session = tx
            .query_row(
                "SELECT session_id, project_root, connection_id, model_id, tool_mode,
                        settings_revision_id, created_at, updated_at, status, display_name,
                        objective, parent_session_id, parent_branch_id, parent_turn_id
                 FROM sessions WHERE session_id = ?1",
                params![session_id.to_string()],
                parse_session_record,
            )
            .map_err(storage_error)?;
        tx.commit().map_err(storage_error)?;
        Ok(CommittedSessionSettingsUpdate {
            session,
            event,
            seq_id,
        })
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

        if Self::session_budget_exhausted_in_tx(&tx, session_id)? {
            return Ok(SessionTurnAdmission::BudgetExhausted);
        }

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
        if Self::session_budget_exhausted_in_tx(&tx, session_id)? {
            return Ok(ContinuationClaim::BudgetExhausted);
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
        if Self::session_budget_exhausted_in_tx(&tx, session_id)? {
            return Ok(ContinuationClaim::BudgetExhausted);
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

    pub fn claim_related_message_continuation(
        &mut self,
        session_id: SessionId,
        message_id: RelatedSessionMessageId,
        expected_settings_revision_id: u64,
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
        if Self::session_budget_exhausted_in_tx(&tx, session_id)? {
            return Ok(ContinuationClaim::BudgetExhausted);
        }
        let current_settings_revision_id = tx
            .query_row(
                "SELECT settings_revision_id FROM sessions WHERE session_id = ?1",
                params![session_id.to_string()],
                |row| row.get::<_, u64>(0),
            )
            .optional()
            .map_err(storage_error)?;
        if current_settings_revision_id != Some(expected_settings_revision_id) {
            return Ok(ContinuationClaim::Stale);
        }
        let pending = tx
            .query_row(
                "SELECT message_id, message_json FROM related_session_message_projection
                 WHERE session_id = ?1 AND direction = 'received' AND status = 'pending'
                 ORDER BY source_seq ASC LIMIT 1",
                params![session_id.to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(storage_error)?;
        let Some((pending_message_id, message_json)) = pending else {
            return Ok(ContinuationClaim::Stale);
        };
        if pending_message_id != message_id.to_string() {
            return Ok(ContinuationClaim::Stale);
        }
        let message: RelatedSessionMessage = serde_json::from_str(&message_json)?;
        if events.len() != 2
            || events.iter().any(|event| {
                event.session_id != session_id || event.branch_id != message.destination_branch_id
            })
            || !matches!(
                &events[0].payload,
                EventPayload::RelatedSessionMessageResolved {
                    message_id: event_message_id,
                    status: RelatedSessionMessageStatus::Claimed,
                    resulting_turn_id: Some(turn_id),
                    ..
                } if *event_message_id == message_id && events[1].turn_id == Some(*turn_id)
            )
            || !matches!(
                &events[1].payload,
                EventPayload::TurnStarted {
                    source: TurnStartSource::RelatedSessionMessage,
                    settings_revision_id,
                    ..
                } if *settings_revision_id == expected_settings_revision_id
            )
        {
            return Err(BelltowerError::InvalidState(
                "related-session continuation events are malformed".to_owned(),
            ));
        }
        let seq_ids = Self::append_events_in_tx(&tx, events)?;
        tx.commit().map_err(storage_error)?;
        Ok(ContinuationClaim::Claimed { seq_ids })
    }

    pub fn claim_resumed_continuation(
        &mut self,
        session_id: SessionId,
        branch_id: BranchId,
        call_id: &ToolCallId,
        tool_name: &str,
        expected_turn_id: TurnId,
        expected_request_seq_id: i64,
        expected_settings_revision_id: u64,
        kind: ResumedContinuationKind,
        events: &[EventEnvelope],
    ) -> Result<ContinuationClaim> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        if Self::session_budget_exhausted_in_tx(&tx, session_id)? {
            return Ok(ContinuationClaim::BudgetExhausted);
        }
        if Self::cancel_requested_in_tx(&tx, session_id)? {
            return Ok(ContinuationClaim::CancelPending);
        }
        if Self::active_turn_exists_in_tx(&tx, session_id)? {
            return Ok(ContinuationClaim::Busy);
        }
        let pending = match kind {
            ResumedContinuationKind::Approval => tx.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM approval_projection
                    WHERE session_id = ?1 AND call_id = ?2 AND tool_name = ?3 AND status = 'pending'
                      AND branch_id = ?4 AND turn_id = ?5 AND requested_seq_id = ?6
                )",
                params![session_id.to_string(), call_id.to_string(), tool_name, branch_id.to_string(), expected_turn_id.to_string(), expected_request_seq_id],
                |row| row.get::<_, i64>(0),
            ),
            ResumedContinuationKind::Input => tx.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM tool_run_projection
                    WHERE session_id = ?1 AND call_id = ?2 AND tool_name = ?3 AND status = 'requested'
                      AND branch_id = ?4 AND turn_id = ?5 AND requested_seq_id = ?6
                )",
                params![session_id.to_string(), call_id.to_string(), tool_name, branch_id.to_string(), expected_turn_id.to_string(), expected_request_seq_id],
                |row| row.get::<_, i64>(0),
            ),
        }
        .map_err(storage_error)?
            != 0;
        if !pending {
            return Ok(ContinuationClaim::Stale);
        }
        validate_resumed_continuation_events(
            session_id,
            branch_id,
            call_id,
            expected_settings_revision_id,
            kind,
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
        if let Some(turn_id) = turn_id {
            Self::ensure_active_turn_owner_in_tx(&tx, session_id, branch_id, turn_id)?;
        }
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

    /// Appends one live streamed completion delta atomically: the raw
    /// provider chunk row (when present) with its `raw.chunk` event, plus the
    /// `completion.chunk` event, in a single transaction. Live streaming
    /// previously paid two lock/transaction/fsync cycles per delta.
    /// Returned envelopes carry their assigned seq ids, in append order.
    #[allow(clippy::too_many_arguments)]
    pub fn append_live_completion_chunk<R, C>(
        &mut self,
        session_id: SessionId,
        branch_id: BranchId,
        turn_id: Option<TurnId>,
        llm_call_ordinal: Option<u32>,
        provider: &str,
        stream_name: &str,
        raw_content: Option<&[u8]>,
        build_raw_event: R,
        build_chunk_event: C,
    ) -> Result<(Option<i64>, Vec<EventEnvelope>)>
    where
        R: FnOnce(i64) -> Result<EventEnvelope>,
        C: FnOnce(Option<i64>) -> Result<EventEnvelope>,
    {
        let tx = self.connection.transaction().map_err(storage_error)?;
        if let Some(turn_id) = turn_id {
            Self::ensure_active_turn_owner_in_tx(&tx, session_id, branch_id, turn_id)?;
        }
        let mut events = Vec::with_capacity(2);
        let raw_chunk_index = if let Some(content) = raw_content {
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
            let mut raw_event = build_raw_event(chunk_id)?;
            tx.execute(
                "UPDATE raw_chunks SET event_id = ?1 WHERE chunk_id = ?2",
                params![raw_event.event_id.to_string(), chunk_id],
            )
            .map_err(storage_error)?;
            raw_event.seq_id = Some(Self::append_event_in_tx(&tx, &raw_event)?);
            events.push(raw_event);
            Some(chunk_id)
        } else {
            None
        };
        let mut chunk_event = build_chunk_event(raw_chunk_index)?;
        chunk_event.seq_id = Some(Self::append_event_in_tx(&tx, &chunk_event)?);
        events.push(chunk_event);
        tx.commit().map_err(storage_error)?;
        Ok((raw_chunk_index, events))
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
        let tx = self.connection.transaction().map_err(storage_error)?;
        if let Some(turn_id) = turn_id {
            Self::ensure_active_turn_owner_in_tx(&tx, session_id, branch_id, turn_id)?;
        }
        tx.execute(
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
        let chunk_id = tx.last_insert_rowid();
        tx.commit().map_err(storage_error)?;
        Ok(chunk_id)
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
        tx.prepare_cached(
            "INSERT INTO events (
                event_id, session_id, branch_id, span_id, parent_span_id, span_kind, event_kind, event_json, occurred_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .map_err(storage_error)?
        .execute(params![
            event.event_id.to_string(),
            event.session_id.to_string(),
            event.branch_id.to_string(),
            event.span_id.to_string(),
            event.parent_span_id.map(|value| value.to_string()),
            span_kind_to_str(&event.span_kind),
            event.kind(),
            event_json,
            format_time(event.occurred_at)?,
        ])
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

    fn ensure_active_turn_owner_in_tx(
        tx: &Transaction<'_>,
        session_id: SessionId,
        branch_id: BranchId,
        turn_id: TurnId,
    ) -> Result<()> {
        let owns_active_turn = tx
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM active_turn_projection
                    WHERE session_id = ?1 AND branch_id = ?2 AND turn_id = ?3
                )",
                params![
                    session_id.to_string(),
                    branch_id.to_string(),
                    turn_id.to_string(),
                ],
                |row| row.get::<_, i64>(0),
            )
            .map_err(storage_error)?
            != 0;
        if !owns_active_turn {
            return Err(BelltowerError::InvalidState(
                "terminal transition does not own the active turn".to_owned(),
            ));
        }
        Ok(())
    }

    fn validate_active_turn_events(
        session_id: SessionId,
        branch_id: BranchId,
        turn_id: TurnId,
        events: &[EventEnvelope],
    ) -> Result<()> {
        if events.is_empty()
            || events.iter().any(|event| {
                event.session_id != session_id
                    || event.branch_id != branch_id
                    || event.turn_id != Some(turn_id)
                    || matches!(&event.payload, EventPayload::TurnFinished { .. })
            })
        {
            return Err(BelltowerError::InvalidState(
                "active turn transition must contain non-terminal events for exactly one turn"
                    .to_owned(),
            ));
        }
        Ok(())
    }

    fn validate_turn_terminal_events(
        session_id: SessionId,
        branch_id: BranchId,
        turn_id: TurnId,
        terminal_events: &[EventEnvelope],
    ) -> Result<()> {
        if terminal_events.iter().any(|event| {
            event.session_id != session_id
                || event.branch_id != branch_id
                || event.turn_id != Some(turn_id)
        }) || !matches!(
            terminal_events.last().map(|event| &event.payload),
            Some(EventPayload::TurnFinished {
                turn_id: finished_turn_id,
                ..
            }) if *finished_turn_id == turn_id
        ) {
            return Err(BelltowerError::InvalidState(
                "terminal transition must contain one turn and end with turn.finished".to_owned(),
            ));
        }
        Ok(())
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

    fn session_budget_exhausted_in_tx(tx: &Transaction<'_>, session_id: SessionId) -> Result<bool> {
        let projection = tx
            .query_row(
                "SELECT max_wall_clock_seconds, max_tokens, max_turns, max_cost_usd,
                        tokens_used, turns_used, elapsed_seconds, cost_used_usd
                 FROM session_budget_projection WHERE session_id = ?1",
                params![session_id.to_string()],
                |row| {
                    Ok((
                        BudgetConfig {
                            max_wall_clock_seconds: row
                                .get::<_, Option<i64>>(0)?
                                .map(|value| u64::try_from(value).map_err(to_sql_conversion))
                                .transpose()?,
                            max_tokens: row
                                .get::<_, Option<i64>>(1)?
                                .map(|value| u64::try_from(value).map_err(to_sql_conversion))
                                .transpose()?,
                            max_turns: row
                                .get::<_, Option<i64>>(2)?
                                .map(|value| u32::try_from(value).map_err(to_sql_conversion))
                                .transpose()?,
                            max_cost_usd: row.get(3)?,
                        },
                        row.get::<_, i64>(4)?.max(0) as u64,
                        row.get::<_, i64>(5)?.max(0) as u32,
                        row.get::<_, i64>(6)?.max(0) as u64,
                        row.get::<_, Option<f64>>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_error)?;
        Ok(projection.is_some_and(
            |(budget, tokens_used, turns_used, elapsed_seconds, cost_used_usd)| {
                budget.is_exhausted(tokens_used, turns_used, elapsed_seconds, cost_used_usd)
            },
        ))
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

    pub fn load_related_session_messages(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<RelatedMessageProjection>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT message_json, direction, status, event_id, counterpart_event_id,
                        source_seq, resulting_turn_id, resolved_at
                 FROM related_session_message_projection
                 WHERE session_id = ?1 ORDER BY source_seq ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![session_id.to_string()],
                parse_related_message_record,
            )
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub fn load_pending_related_session_messages(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<RelatedMessageProjection>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT message_json, direction, status, event_id, counterpart_event_id,
                        source_seq, resulting_turn_id, resolved_at
                 FROM related_session_message_projection
                 WHERE session_id = ?1 AND direction = 'received' AND status = 'pending'
                 ORDER BY source_seq ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![session_id.to_string()],
                parse_related_message_record,
            )
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub fn load_all_pending_related_session_messages(
        &self,
    ) -> Result<Vec<RelatedMessageProjection>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT message_json, direction, status, event_id, counterpart_event_id,
                        source_seq, resulting_turn_id, resolved_at
                 FROM related_session_message_projection
                 WHERE direction = 'received' AND status = 'pending'
                 ORDER BY source_seq ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], parse_related_message_record)
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

    /// Loads only events of one kind (optionally branch-scoped) through the
    /// `events(session_id, event_kind, seq_id)` index, so control-flow
    /// lookups (latest turn boundary, finished-turn checks) stop replaying
    /// whole session logs.
    pub fn load_events_of_kind(
        &self,
        session_id: SessionId,
        event_kind: &str,
        branch_id: Option<BranchId>,
    ) -> Result<Vec<EventEnvelope>> {
        let mut statement = self
            .connection
            .prepare_cached(
                "SELECT event_json, seq_id
                 FROM events
                 WHERE session_id = ?1 AND event_kind = ?2
                   AND (?3 IS NULL OR branch_id = ?3)
                 ORDER BY seq_id ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![
                    session_id.to_string(),
                    event_kind,
                    branch_id.map(|id| id.to_string())
                ],
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

    /// The session's current active-turn claim, straight from the
    /// coordination projection.
    pub fn active_turn_claim(&self, session_id: SessionId) -> Result<Option<(BranchId, TurnId)>> {
        let mut statement = self
            .connection
            .prepare_cached(
                "SELECT branch_id, turn_id FROM active_turn_projection WHERE session_id = ?1",
            )
            .map_err(storage_error)?;
        let mut rows = statement
            .query_map(params![session_id.to_string()], |row| {
                let branch_id: String = row.get(0)?;
                let turn_id: String = row.get(1)?;
                Ok((
                    branch_id.parse().map_err(to_sql_conversion)?,
                    turn_id.parse().map_err(to_sql_conversion)?,
                ))
            })
            .map_err(storage_error)?;
        rows.next().transpose().map_err(storage_error)
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

    /// Per-turn inspection summaries from the incrementally maintained
    /// `turn_projection` read model, ordered by each turn's first event
    /// sequence. This is the projection behind turn history; it never
    /// replays the event log.
    pub fn load_turn_projections(&self, session_id: SessionId) -> Result<Vec<TurnInspection>> {
        crate::turn_projection::load_turn_projections_ordered(
            &self.connection,
            &session_id.to_string(),
        )
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
                "SELECT call_id, session_id, tool_name, status, branch_id, turn_id, requested_seq_id, request_fingerprint, request_snapshot_json, decision_json, resolution_json, updated_at
             FROM approval_projection
             WHERE session_id = ?1
             ORDER BY updated_at ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![session_id.to_string()], |row| {
                let request_snapshot_json: Option<String> = row.get(8)?;
                let decision_json: Option<String> = row.get(9)?;
                let resolution_json: Option<String> = row.get(10)?;
                Ok(ApprovalProjection {
                    call_id: ToolCallId::new(row.get::<_, String>(0)?),
                    session_id: SessionId(
                        row.get::<_, String>(1)?
                            .parse()
                            .map_err(to_sql_conversion)?,
                    ),
                    tool_name: row.get(2)?,
                    status: row.get(3)?,
                    branch_id: row
                        .get::<_, Option<String>>(4)?
                        .map(|value| value.parse().map(BranchId))
                        .transpose()
                        .map_err(to_sql_conversion)?,
                    turn_id: row
                        .get::<_, Option<String>>(5)?
                        .map(|value| value.parse().map(TurnId))
                        .transpose()
                        .map_err(to_sql_conversion)?,
                    requested_seq_id: row.get(6)?,
                    request_fingerprint: row.get(7)?,
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
                    updated_at: parse_time(row.get::<_, String>(11)?)?,
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
            "SELECT call_id, session_id, tool_name, status, branch_id, turn_id, requested_seq_id, arguments_json, result_json, updated_at
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
                    branch_id: row
                        .get::<_, Option<String>>(4)?
                        .map(|value| value.parse().map(BranchId))
                        .transpose()
                        .map_err(to_sql_conversion)?,
                    turn_id: row
                        .get::<_, Option<String>>(5)?
                        .map(|value| value.parse().map(TurnId))
                        .transpose()
                        .map_err(to_sql_conversion)?,
                    requested_seq_id: row.get(6)?,
                    arguments: row
                        .get::<_, Option<String>>(7)?
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(to_sql_conversion)?,
                    result: row
                        .get::<_, Option<String>>(8)?
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(to_sql_conversion)?,
                    updated_at: parse_time(row.get::<_, String>(9)?)?,
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
                    turns_used,
                    elapsed_seconds,
                    cost_used_usd,
                } => Self::refresh_budget_projection_from_checkpoint(
                    &tx,
                    stored.event.session_id,
                    stored.event.occurred_at,
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
