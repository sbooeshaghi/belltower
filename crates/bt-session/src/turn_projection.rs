//! Shared fold and row codec for the `turn_projection` read model.
//!
//! The canonical event log stays the source of truth: every appended event
//! that resolves to a turn (envelope turn id or a turn-bearing payload) folds
//! into one durable per-turn summary row. The same fold drives the
//! incremental per-event refresh inside the append transaction
//! (`store::projections`) and the marker-gated full rebuild in `migration`,
//! so the projection cannot drift from replay semantics. Inspection surfaces
//! read these rows instead of replaying session logs.

use bt_core::{
    ApprovalDecision, EventEnvelope, EventPayload, ToolCallId, TurnId, TurnInspection,
    TurnToolCallSummary, default_settings_revision_id,
};
use rusqlite::{Connection, OptionalExtension, params};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const TURN_PROJECTION_COLUMNS: &str = "branch_id, turn_id, provider, model, message_count, \
     settings_revision_id, started_at, finished_at, status, finish_reason, latency_ms, \
     event_seq_start, event_seq_end, event_count, pending_approval_count, raw_chunk_count, \
     tool_calls_json";

fn write_conversion<E>(error: E) -> rusqlite::Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

fn read_conversion<E>(error: E) -> rusqlite::Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

/// The turn an event aggregates into: the envelope turn id, falling back to
/// turn ids carried by turn-boundary payloads. Events without a resolvable
/// turn id do not touch the projection.
pub(crate) fn projected_turn_id(event: &EventEnvelope) -> Option<TurnId> {
    event.turn_id.or_else(|| match &event.payload {
        EventPayload::TurnStarted { turn_id, .. } | EventPayload::TurnFinished { turn_id, .. } => {
            Some(*turn_id)
        }
        EventPayload::TurnInstructionProvenanceRecorded { provenance } => Some(provenance.turn_id),
        _ => None,
    })
}

/// A fresh summary for a turn's first observed event. Mirrors the historical
/// replay fold: branch and started-at come from that first event, and a later
/// `turn.started` overwrites provider/model/settings/started-at/seq-start.
pub(crate) fn new_turn_projection(
    event: &EventEnvelope,
    turn_id: TurnId,
    seq_id: i64,
) -> TurnInspection {
    TurnInspection {
        turn_id,
        branch_id: event.branch_id,
        provider: String::new(),
        model: String::new(),
        message_count: 0,
        settings_revision_id: default_settings_revision_id(),
        started_at: event.occurred_at,
        finished_at: None,
        status: None,
        finish_reason: None,
        latency_ms: None,
        event_seq_start: Some(seq_id),
        event_seq_end: Some(seq_id),
        event_count: 0,
        pending_approval_count: 0,
        raw_chunk_count: 0,
        tool_calls: Vec::new(),
    }
}

/// One fold step over a turn-bound event. This mirrors the pre-projection
/// `turn_history` replay fold exactly; a semantic change here must ship with
/// a new rebuild marker in `migration.rs` so existing rows are refolded.
pub(crate) fn apply_turn_projection_event(
    turn: &mut TurnInspection,
    event: &EventEnvelope,
    seq_id: i64,
) {
    turn.event_count += 1;
    if turn.event_seq_start.is_none() {
        turn.event_seq_start = Some(seq_id);
    }
    turn.event_seq_end = Some(seq_id);

    match &event.payload {
        EventPayload::TurnStarted {
            provider,
            model,
            message_count,
            settings_revision_id,
            ..
        } => {
            turn.provider = provider.clone();
            turn.model = model.clone();
            turn.message_count = *message_count;
            turn.settings_revision_id = *settings_revision_id;
            turn.started_at = event.occurred_at;
            turn.event_seq_start = Some(seq_id);
        }
        EventPayload::CompletionRequested {
            llm_call_ordinal: _,
            provider,
            model,
            message_count,
        } => {
            if turn.provider.is_empty() {
                turn.provider = provider.clone();
            }
            if turn.model.is_empty() {
                turn.model = model.clone();
            }
            if turn.message_count == 0 {
                turn.message_count = *message_count;
            }
        }
        EventPayload::SessionError { code, .. } => {
            if turn.finish_reason.is_none() {
                turn.finish_reason = Some(code.clone());
            }
        }
        EventPayload::TurnFinished {
            provider,
            model,
            status,
            finish_reason,
            latency_ms,
            ..
        } => {
            turn.provider = provider.clone();
            turn.model = model.clone();
            turn.status = Some(status.clone());
            turn.finish_reason = finish_reason.clone();
            turn.latency_ms = Some(*latency_ms);
            turn.finished_at = Some(event.occurred_at);
        }
        EventPayload::RawChunkPersisted { .. } => {
            turn.raw_chunk_count += 1;
        }
        EventPayload::ToolCallRequested {
            call_id, tool_name, ..
        } => {
            let tool = ensure_tool_call(turn, call_id.clone(), tool_name.clone());
            tool.tool_name = tool_name.clone();
            tool.approval_status = None;
            tool.execution_status = None;
        }
        EventPayload::ToolApprovalRequested {
            call_id, tool_name, ..
        } => {
            let tool = ensure_tool_call(turn, call_id.clone(), tool_name.clone());
            tool.approval_status = Some("pending".to_owned());
            turn.pending_approval_count += 1;
        }
        EventPayload::ToolApprovalResolved {
            call_id,
            tool_name,
            decision,
            ..
        } => {
            let tool = ensure_tool_call(turn, call_id.clone(), tool_name.clone());
            tool.approval_status = Some(match decision {
                ApprovalDecision::Approved { .. } => "approved".to_owned(),
                ApprovalDecision::Denied { .. } => "denied".to_owned(),
            });
            if turn.pending_approval_count > 0 {
                turn.pending_approval_count -= 1;
            }
        }
        EventPayload::ToolExecutionFinished {
            call_id,
            tool_name,
            result,
        } => {
            let tool = ensure_tool_call(turn, call_id.clone(), tool_name.clone());
            tool.execution_status = Some(if result.is_error {
                "error".to_owned()
            } else {
                "completed".to_owned()
            });
        }
        _ => {}
    }
}

fn ensure_tool_call(
    turn: &mut TurnInspection,
    call_id: ToolCallId,
    tool_name: String,
) -> &mut TurnToolCallSummary {
    if let Some(index) = turn
        .tool_calls
        .iter()
        .position(|tool| tool.call_id == call_id)
    {
        return &mut turn.tool_calls[index];
    }

    turn.tool_calls.push(TurnToolCallSummary {
        call_id,
        tool_name,
        approval_status: None,
        execution_status: None,
    });
    turn.tool_calls
        .last_mut()
        .expect("tool call exists immediately after push")
}

pub(crate) fn load_turn_projection(
    connection: &Connection,
    session_id: &str,
    turn_id: &str,
) -> rusqlite::Result<Option<TurnInspection>> {
    connection
        .query_row(
            &format!(
                "SELECT {TURN_PROJECTION_COLUMNS}
                 FROM turn_projection
                 WHERE session_id = ?1 AND turn_id = ?2"
            ),
            params![session_id, turn_id],
            parse_turn_projection_row,
        )
        .optional()
}

pub(crate) fn load_turn_projections_ordered(
    connection: &Connection,
    session_id: &str,
) -> rusqlite::Result<Vec<TurnInspection>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {TURN_PROJECTION_COLUMNS}
         FROM turn_projection
         WHERE session_id = ?1
         ORDER BY event_seq_start ASC"
    ))?;
    let rows = statement.query_map(params![session_id], parse_turn_projection_row)?;
    rows.collect()
}

pub(crate) fn upsert_turn_projection(
    connection: &Connection,
    session_id: &str,
    turn: &TurnInspection,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT OR REPLACE INTO turn_projection (
            session_id, branch_id, turn_id, provider, model, message_count,
            settings_revision_id, started_at, finished_at, status, finish_reason,
            latency_ms, event_seq_start, event_seq_end, event_count,
            pending_approval_count, raw_chunk_count, tool_calls_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            session_id,
            turn.branch_id.to_string(),
            turn.turn_id.to_string(),
            turn.provider,
            turn.model,
            i64::from(turn.message_count),
            turn.settings_revision_id as i64,
            format_projection_time(turn.started_at)?,
            turn.finished_at.map(format_projection_time).transpose()?,
            turn.status,
            turn.finish_reason,
            turn.latency_ms.map(|value| value as i64),
            turn.event_seq_start,
            turn.event_seq_end,
            i64::from(turn.event_count),
            i64::from(turn.pending_approval_count),
            i64::from(turn.raw_chunk_count),
            serde_json::to_string(&turn.tool_calls).map_err(write_conversion)?,
        ],
    )?;
    Ok(())
}

fn parse_turn_projection_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TurnInspection> {
    let branch_id: String = row.get(0)?;
    let turn_id: String = row.get(1)?;
    let message_count: i64 = row.get(4)?;
    let settings_revision_id: i64 = row.get(5)?;
    let started_at: String = row.get(6)?;
    let finished_at: Option<String> = row.get(7)?;
    let latency_ms: Option<i64> = row.get(10)?;
    let event_count: i64 = row.get(13)?;
    let pending_approval_count: i64 = row.get(14)?;
    let raw_chunk_count: i64 = row.get(15)?;
    let tool_calls_json: String = row.get(16)?;
    Ok(TurnInspection {
        turn_id: turn_id.parse().map_err(read_conversion)?,
        branch_id: branch_id.parse().map_err(read_conversion)?,
        provider: row.get(2)?,
        model: row.get(3)?,
        message_count: u32::try_from(message_count).map_err(read_conversion)?,
        settings_revision_id: u64::try_from(settings_revision_id).map_err(read_conversion)?,
        started_at: parse_projection_time(&started_at)?,
        finished_at: finished_at
            .map(|value| parse_projection_time(&value))
            .transpose()?,
        status: row.get(8)?,
        finish_reason: row.get(9)?,
        latency_ms: latency_ms
            .map(|value| u64::try_from(value).map_err(read_conversion))
            .transpose()?,
        event_seq_start: row.get(11)?,
        event_seq_end: row.get(12)?,
        event_count: u32::try_from(event_count).map_err(read_conversion)?,
        pending_approval_count: u32::try_from(pending_approval_count).map_err(read_conversion)?,
        raw_chunk_count: u32::try_from(raw_chunk_count).map_err(read_conversion)?,
        tool_calls: serde_json::from_str(&tool_calls_json).map_err(read_conversion)?,
    })
}

fn format_projection_time(value: OffsetDateTime) -> rusqlite::Result<String> {
    value.format(&Rfc3339).map_err(write_conversion)
}

fn parse_projection_time(value: &str) -> rusqlite::Result<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(read_conversion)
}
