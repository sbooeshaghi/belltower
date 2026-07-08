//! Row parsing, enum encoding, and storage error conversion helpers.
//!
//! This module centralizes SQLite serialization details so store operations and
//! projection refresh code can stay focused on durable state transitions.

use crate::StoredSessionEvent;
use bt_core::{
    ApprovalDecision, BelltowerError, BranchId, BranchRecord, EventEnvelope,
    QueuedMessageResolutionOutcome, Result, SessionId, SessionRecord, SessionStatus,
    SessionToolMode, SteerResolutionOutcome, TurnId,
};
use rusqlite::{Connection, OptionalExtension, params};
use time::OffsetDateTime;

use super::RawChunkRecord;

pub(super) fn parse_session_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    Ok(SessionRecord {
        session_id: bt_core::SessionId(
            row.get::<_, String>(0)?
                .parse()
                .map_err(to_sql_conversion)?,
        ),
        project_root: row.get::<_, String>(1)?.into(),
        connection_id: bt_core::ConnectionId::new(row.get::<_, String>(2)?),
        model_id: row.get(3)?,
        tool_mode: parse_session_tool_mode(row.get::<_, String>(4)?),
        settings_revision_id: row.get::<_, i64>(5)?.max(1) as u64,
        created_at: parse_time(row.get::<_, String>(6)?)?,
        updated_at: parse_time(row.get::<_, String>(7)?)?,
        status: parse_status(row.get::<_, String>(8)?),
        display_name: row.get(9)?,
        objective: row.get(10)?,
        parent_session_id: row
            .get::<_, Option<String>>(11)?
            .map(|value| value.parse().map(SessionId).map_err(to_sql_conversion))
            .transpose()?,
        parent_branch_id: row
            .get::<_, Option<String>>(12)?
            .map(|value| value.parse().map(BranchId).map_err(to_sql_conversion))
            .transpose()?,
        parent_turn_id: row
            .get::<_, Option<String>>(13)?
            .map(|value| {
                value
                    .parse()
                    .map(bt_core::TurnId)
                    .map_err(to_sql_conversion)
            })
            .transpose()?,
    })
}

pub(super) fn count_query(
    connection: &Connection,
    query: &str,
    session_id: SessionId,
) -> Result<u32> {
    let count: i64 = connection
        .query_row(query, params![session_id.to_string()], |row| row.get(0))
        .map_err(storage_error)?;
    Ok(count.max(0) as u32)
}

pub(super) fn optional_i64_query(
    connection: &Connection,
    query: &str,
    session_id: SessionId,
) -> Result<Option<i64>> {
    connection
        .query_row(query, params![session_id.to_string()], |row| row.get(0))
        .optional()
        .map_err(storage_error)
        .map(|value: Option<Option<i64>>| value.flatten())
}

pub(super) fn parse_branch_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<BranchRecord> {
    Ok(BranchRecord {
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
        parent_branch_id: row
            .get::<_, Option<String>>(2)?
            .map(|value| value.parse().map(BranchId).map_err(to_sql_conversion))
            .transpose()?,
        parent_event_id: row
            .get::<_, Option<String>>(3)?
            .map(|value| {
                value
                    .parse()
                    .map(bt_core::EventId)
                    .map_err(to_sql_conversion)
            })
            .transpose()?,
        head_event_id: row
            .get::<_, Option<String>>(4)?
            .map(|value| {
                value
                    .parse()
                    .map(bt_core::EventId)
                    .map_err(to_sql_conversion)
            })
            .transpose()?,
        summary: row.get(5)?,
        created_at: parse_time(row.get::<_, String>(6)?)?,
        is_default: row.get(7)?,
    })
}

pub(super) fn parse_raw_chunk_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawChunkRecord> {
    Ok(RawChunkRecord {
        chunk_id: row.get(0)?,
        session_id: SessionId(
            row.get::<_, String>(1)?
                .parse()
                .map_err(to_sql_conversion)?,
        ),
        branch_id: row
            .get::<_, Option<String>>(2)?
            .map(|value| value.parse().map(BranchId).map_err(to_sql_conversion))
            .transpose()?,
        turn_id: row
            .get::<_, Option<String>>(3)?
            .map(|value| value.parse().map(TurnId).map_err(to_sql_conversion))
            .transpose()?,
        llm_call_ordinal: row
            .get::<_, Option<i64>>(4)?
            .map(|value| u32::try_from(value).map_err(to_sql_conversion))
            .transpose()?,
        event_id: row.get(5)?,
        provider: row.get(6)?,
        stream_name: row.get(7)?,
        content: row.get(8)?,
        received_at: parse_time(row.get::<_, String>(9)?)?,
    })
}

pub(super) fn load_session_event_log_from_connection(
    connection: &Connection,
    session_id: SessionId,
) -> Result<Vec<StoredSessionEvent>> {
    let mut statement = connection
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
            let mut event: EventEnvelope = serde_json::from_str(&raw).map_err(to_sql_conversion)?;
            event.seq_id = Some(seq_id);
            Ok(StoredSessionEvent { seq_id, event })
        })
        .map_err(storage_error)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(storage_error)
}

pub(super) fn parse_time(value: String) -> rusqlite::Result<OffsetDateTime> {
    OffsetDateTime::parse(&value, &time::format_description::well_known::Rfc3339)
        .map_err(to_sql_conversion)
}

pub(super) fn session_status_to_str(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Active => "active",
        SessionStatus::Completed => "completed",
        SessionStatus::Failed => "failed",
        SessionStatus::Abandoned => "abandoned",
    }
}

pub(super) fn session_tool_mode_to_str(mode: SessionToolMode) -> &'static str {
    match mode {
        SessionToolMode::Standard => "standard",
        SessionToolMode::Extended => "extended",
    }
}

pub(super) fn parse_session_tool_mode(value: String) -> SessionToolMode {
    match value.as_str() {
        "standard" => SessionToolMode::Standard,
        _ => SessionToolMode::Extended,
    }
}

pub(super) fn parse_status(value: String) -> SessionStatus {
    match value.as_str() {
        "completed" => SessionStatus::Completed,
        "failed" => SessionStatus::Failed,
        "abandoned" => SessionStatus::Abandoned,
        _ => SessionStatus::Active,
    }
}

pub(super) fn span_kind_to_str(kind: &bt_core::SpanKind) -> &'static str {
    match kind {
        bt_core::SpanKind::Session => "session",
        bt_core::SpanKind::Agent => "agent",
        bt_core::SpanKind::Llm => "llm",
        bt_core::SpanKind::Tool => "tool",
        bt_core::SpanKind::Chain => "chain",
    }
}

pub(super) fn approval_status(decision: &ApprovalDecision) -> &'static str {
    match decision {
        ApprovalDecision::Approved { .. } => "approved",
        ApprovalDecision::Denied { .. } => "denied",
    }
}

pub(super) fn queued_message_resolution_status(
    outcome: &QueuedMessageResolutionOutcome,
) -> &'static str {
    match outcome {
        QueuedMessageResolutionOutcome::Dispatched => "dispatched",
        QueuedMessageResolutionOutcome::Dropped => "dropped",
    }
}

pub(super) fn steer_resolution_status(outcome: &SteerResolutionOutcome) -> &'static str {
    match outcome {
        SteerResolutionOutcome::Applied => "applied",
        SteerResolutionOutcome::Dropped => "dropped",
    }
}

pub(super) fn format_time(value: OffsetDateTime) -> Result<String> {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| BelltowerError::Storage(error.to_string()))
}

pub(super) fn to_sql_conversion<E>(error: E) -> rusqlite::Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

pub(super) fn storage_error(error: rusqlite::Error) -> BelltowerError {
    BelltowerError::Storage(error.to_string())
}
