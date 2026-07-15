//! Projection refresh helpers for session-store derived tables.
//!
//! The canonical event log remains the source of truth; these helpers update
//! query projections after events are durably appended or replayed.

use super::{
    SqliteSessionStore, approval_status, format_time, queued_message_resolution_status,
    steer_resolution_status, storage_error,
};
use bt_core::{
    BelltowerError, BudgetConfig, CostBreakdown, EventEnvelope, EventPayload, Result, SessionId,
    TokenUsage,
};
use rusqlite::{OptionalExtension, Transaction, params};
use time::OffsetDateTime;

impl SqliteSessionStore {
    pub(super) fn refresh_cost_summary_projection(
        tx: &Transaction<'_>,
        session_id: SessionId,
        occurred_at: OffsetDateTime,
        usage: &TokenUsage,
        cost: Option<&CostBreakdown>,
    ) -> Result<()> {
        let unpriced_completion_count = u64::from(cost.is_none());
        tx.execute(
            "INSERT INTO cost_summary_projection (session_id, prompt_tokens, completion_tokens, total_tokens, total_cost_usd, unpriced_completion_count, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(session_id) DO UPDATE SET
                prompt_tokens = prompt_tokens + excluded.prompt_tokens,
                completion_tokens = completion_tokens + excluded.completion_tokens,
                total_tokens = total_tokens + excluded.total_tokens,
                total_cost_usd = total_cost_usd + excluded.total_cost_usd,
                unpriced_completion_count = unpriced_completion_count + excluded.unpriced_completion_count,
                updated_at = excluded.updated_at",
            params![
                session_id.to_string(),
                usage.prompt_tokens,
                usage.completion_tokens,
                usage.total_tokens,
                cost.map_or(0.0, |cost| cost.total_usd),
                unpriced_completion_count,
                format_time(occurred_at)?
            ],
        )
        .map_err(storage_error)?;
        Ok(())
    }

    pub(super) fn refresh_budget_projection_from_config(
        tx: &Transaction<'_>,
        session_id: SessionId,
        occurred_at: OffsetDateTime,
        budget: &BudgetConfig,
    ) -> Result<()> {
        tx.execute(
            "INSERT INTO session_budget_projection (
                session_id, max_wall_clock_seconds, max_tokens, max_turns, max_cost_usd,
                tokens_used, turns_used, elapsed_seconds, cost_used_usd, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, 0, 0, 0, 0.0, ?6)
            ON CONFLICT(session_id) DO UPDATE SET
                max_wall_clock_seconds = excluded.max_wall_clock_seconds,
                max_tokens = excluded.max_tokens,
                max_turns = excluded.max_turns,
                max_cost_usd = excluded.max_cost_usd,
                updated_at = excluded.updated_at",
            params![
                session_id.to_string(),
                budget.max_wall_clock_seconds.map(|value| value as i64),
                budget.max_tokens.map(|value| value as i64),
                budget.max_turns.map(i64::from),
                budget.max_cost_usd,
                format_time(occurred_at)?,
            ],
        )
        .map_err(storage_error)?;
        Ok(())
    }

    pub(super) fn refresh_budget_projection_from_checkpoint(
        tx: &Transaction<'_>,
        session_id: SessionId,
        occurred_at: OffsetDateTime,
        tokens_used: u64,
        turns_used: u32,
        elapsed_seconds: u64,
        cost_used_usd: Option<f64>,
    ) -> Result<()> {
        let current = tx
            .query_row(
                "SELECT tokens_used, turns_used, elapsed_seconds, cost_used_usd
                 FROM session_budget_projection WHERE session_id = ?1",
                params![session_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?.max(0) as u64,
                        row.get::<_, i64>(1)?.max(0) as u32,
                        row.get::<_, i64>(2)?.max(0) as u64,
                        row.get::<_, Option<f64>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_error)?
            .ok_or_else(|| {
                BelltowerError::InvalidState(
                    "budget checkpoint requires a prior budget configuration".to_owned(),
                )
            })?;
        let cost_regressed = match (current.3, cost_used_usd) {
            (Some(current), Some(next)) => next < current,
            (None, Some(_)) => true,
            (Some(_), None) | (None, None) => false,
        };
        if tokens_used < current.0
            || turns_used < current.1
            || elapsed_seconds < current.2
            || cost_regressed
        {
            return Err(BelltowerError::InvalidState(
                "budget checkpoint counters must be monotonic".to_owned(),
            ));
        }
        let updated = tx
            .execute(
                "UPDATE session_budget_projection
                 SET tokens_used = ?1, turns_used = ?2, elapsed_seconds = ?3,
                     cost_used_usd = ?4, updated_at = ?5
                 WHERE session_id = ?6",
                params![
                    tokens_used as i64,
                    i64::from(turns_used),
                    elapsed_seconds as i64,
                    cost_used_usd,
                    format_time(occurred_at)?,
                    session_id.to_string(),
                ],
            )
            .map_err(storage_error)?;
        debug_assert_eq!(updated, 1);
        Ok(())
    }

    pub(super) fn refresh_projections(
        tx: &Transaction<'_>,
        event: &EventEnvelope,
        seq_id: i64,
    ) -> Result<()> {
        tx.execute(
            "INSERT INTO branch_heads (branch_id, session_id, head_event_seq, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(branch_id) DO UPDATE SET head_event_seq = excluded.head_event_seq, updated_at = excluded.updated_at",
            params![
                event.branch_id.to_string(),
                event.session_id.to_string(),
                seq_id,
                format_time(event.occurred_at)?
            ],
        )
        .map_err(storage_error)?;

        tx.execute(
            "UPDATE branches SET head_event_id = ?1 WHERE branch_id = ?2",
            params![event.event_id.to_string(), event.branch_id.to_string()],
        )
        .map_err(storage_error)?;

        tx.execute(
            "UPDATE sessions SET updated_at = ?1 WHERE session_id = ?2",
            params![
                format_time(event.occurred_at)?,
                event.session_id.to_string()
            ],
        )
        .map_err(storage_error)?;

        match &event.payload {
            EventPayload::MessageAppended { message } => {
                tx.execute(
                    "INSERT INTO message_projection (event_id, session_id, branch_id, message_id, role, content_json, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        event.event_id.to_string(),
                        event.session_id.to_string(),
                        event.branch_id.to_string(),
                        message.message_id.to_string(),
                        format!("{:?}", message.role),
                        serde_json::to_string(message)?,
                        format_time(message.created_at)?
                    ],
                )
                .map_err(storage_error)?;
            }
            EventPayload::TurnStarted { turn_id, .. } => {
                tx.execute(
                    "INSERT INTO active_turn_projection (
                        session_id, branch_id, turn_id, started_seq_id, started_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        event.session_id.to_string(),
                        event.branch_id.to_string(),
                        turn_id.to_string(),
                        seq_id,
                        format_time(event.occurred_at)?,
                    ],
                )
                .map_err(storage_error)?;
            }
            EventPayload::TurnFinished { turn_id, .. } => {
                tx.execute(
                    "DELETE FROM active_turn_projection
                     WHERE session_id = ?1 AND turn_id = ?2",
                    params![event.session_id.to_string(), turn_id.to_string()],
                )
                .map_err(storage_error)?;
            }
            EventPayload::TurnContextManifestRecorded { manifest } => {
                tx.execute(
                    "INSERT INTO context_manifest_projection (
                        session_id, branch_id, turn_id, llm_call_ordinal, provider, model,
                        settings_revision_id, context_boundary_seq_id, message_count, tool_count,
                        attachment_count, compacted, manifest_json, recorded_event_id,
                        recorded_seq_id, recorded_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                    ON CONFLICT(session_id, turn_id, llm_call_ordinal) DO UPDATE SET
                        branch_id = excluded.branch_id,
                        provider = excluded.provider,
                        model = excluded.model,
                        settings_revision_id = excluded.settings_revision_id,
                        context_boundary_seq_id = excluded.context_boundary_seq_id,
                        message_count = excluded.message_count,
                        tool_count = excluded.tool_count,
                        attachment_count = excluded.attachment_count,
                        compacted = excluded.compacted,
                        manifest_json = excluded.manifest_json,
                        recorded_event_id = excluded.recorded_event_id,
                        recorded_seq_id = excluded.recorded_seq_id,
                        recorded_at = excluded.recorded_at",
                    params![
                        event.session_id.to_string(),
                        event.branch_id.to_string(),
                        manifest.turn_id.to_string(),
                        i64::from(manifest.llm_call_ordinal),
                        manifest.provider.as_str(),
                        manifest.model.as_str(),
                        manifest.settings_revision_id as i64,
                        manifest.context_boundary_seq_id,
                        manifest.messages.len() as i64,
                        manifest.tools.len() as i64,
                        manifest.attachments.len() as i64,
                        i64::from(manifest.compacted),
                        serde_json::to_string(manifest)?,
                        event.event_id.to_string(),
                        seq_id,
                        format_time(event.occurred_at)?,
                    ],
                )
                .map_err(storage_error)?;
            }
            EventPayload::ToolApprovalRequested {
                call_id,
                tool_name,
                snapshot,
            } => {
                let request_fingerprint = snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.request_fingerprint.as_str());
                let request_snapshot_json =
                    snapshot.as_ref().map(serde_json::to_string).transpose()?;
                tx.execute(
                    "INSERT INTO approval_projection (session_id, call_id, tool_name, status, branch_id, turn_id, requested_seq_id, request_fingerprint, request_snapshot_json, decision_json, resolution_json, updated_at)
                     VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?6, ?7, ?8, NULL, NULL, ?9)
                     ON CONFLICT(session_id, call_id) DO UPDATE SET status = 'pending', tool_name = excluded.tool_name, branch_id = excluded.branch_id, turn_id = excluded.turn_id, requested_seq_id = excluded.requested_seq_id, request_fingerprint = excluded.request_fingerprint, request_snapshot_json = excluded.request_snapshot_json, decision_json = NULL, resolution_json = NULL, updated_at = excluded.updated_at",
                    params![
                        event.session_id.to_string(),
                        call_id.to_string(),
                        tool_name,
                        event.branch_id.to_string(),
                        event.turn_id.map(|turn_id| turn_id.to_string()),
                        seq_id,
                        request_fingerprint,
                        request_snapshot_json,
                        format_time(event.occurred_at)?
                    ],
                )
                .map_err(storage_error)?;
            }
            EventPayload::ToolApprovalResolved {
                call_id,
                tool_name,
                request_fingerprint,
                resolution,
                decision,
            } => {
                let resolved_fingerprint = resolution
                    .as_ref()
                    .map(|resolution| resolution.request_fingerprint.as_str())
                    .or(request_fingerprint.as_deref());
                let resolution_json = resolution.as_ref().map(serde_json::to_string).transpose()?;
                tx.execute(
                    "INSERT INTO approval_projection (session_id, call_id, tool_name, status, request_fingerprint, decision_json, resolution_json, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(session_id, call_id) DO UPDATE SET status = excluded.status, request_fingerprint = excluded.request_fingerprint, decision_json = excluded.decision_json, resolution_json = excluded.resolution_json, updated_at = excluded.updated_at",
                    params![
                        event.session_id.to_string(),
                        call_id.to_string(),
                        tool_name,
                        approval_status(decision),
                        resolved_fingerprint,
                        serde_json::to_string(decision)?,
                        resolution_json,
                        format_time(event.occurred_at)?
                    ],
                )
                .map_err(storage_error)?;
            }
            EventPayload::ToolCallRequested {
                call_id,
                tool_name,
                arguments,
            } => {
                tx.execute(
                    "DELETE FROM approval_projection WHERE session_id = ?1 AND call_id = ?2",
                    params![event.session_id.to_string(), call_id.to_string()],
                )
                .map_err(storage_error)?;
                tx.execute(
                    "INSERT INTO tool_run_projection (session_id, call_id, tool_name, status, branch_id, turn_id, requested_seq_id, arguments_json, result_json, updated_at)
                     VALUES (?1, ?2, ?3, 'requested', ?4, ?5, ?6, ?7, NULL, ?8)
                     ON CONFLICT(session_id, call_id) DO UPDATE SET tool_name = excluded.tool_name, status = 'requested', branch_id = excluded.branch_id, turn_id = excluded.turn_id, requested_seq_id = excluded.requested_seq_id, arguments_json = excluded.arguments_json, result_json = NULL, updated_at = excluded.updated_at",
                    params![
                        event.session_id.to_string(),
                        call_id.to_string(),
                        tool_name,
                        event.branch_id.to_string(),
                        event.turn_id.map(|turn_id| turn_id.to_string()),
                        seq_id,
                        serde_json::to_string(arguments)?,
                        format_time(event.occurred_at)?
                    ],
                )
                .map_err(storage_error)?;
            }
            EventPayload::ToolExecutionFinished {
                call_id,
                tool_name,
                result,
            } => {
                tx.execute(
                    "INSERT INTO tool_run_projection (session_id, call_id, tool_name, status, arguments_json, result_json, updated_at)
                     VALUES (?1, ?2, ?3, 'completed', NULL, ?4, ?5)
                     ON CONFLICT(session_id, call_id) DO UPDATE SET status = 'completed', result_json = excluded.result_json, updated_at = excluded.updated_at",
                    params![
                        event.session_id.to_string(),
                        call_id.to_string(),
                        tool_name,
                        serde_json::to_string(result)?,
                        format_time(event.occurred_at)?
                    ],
                )
                .map_err(storage_error)?;
            }
            EventPayload::PlanUpdated { items } => {
                tx.execute(
                    "INSERT INTO plan_projection (session_id, branch_id, items_json, updated_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(session_id, branch_id) DO UPDATE SET
                        items_json = excluded.items_json,
                        updated_at = excluded.updated_at",
                    params![
                        event.session_id.to_string(),
                        event.branch_id.to_string(),
                        serde_json::to_string(items)?,
                        format_time(event.occurred_at)?
                    ],
                )
                .map_err(storage_error)?;
            }
            EventPayload::SessionSettingsUpdated {
                settings_revision_id,
                connection_id,
                model_id,
                tool_mode,
            } => {
                if *settings_revision_id == 0 {
                    return Err(BelltowerError::InvalidState(
                        "session settings revision must be positive".to_owned(),
                    ));
                }
                let current_settings_revision_id = tx
                    .query_row(
                        "SELECT settings_revision_id
                         FROM sessions
                         WHERE session_id = ?1",
                        params![event.session_id.to_string()],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(storage_error)?
                    .max(1) as u64;
                let connection_id = connection_id.clone();
                let model_id = model_id.clone();
                let tool_mode = tool_mode.clone();
                let existing = tx
                    .query_row(
                        "SELECT connection_id, model_id, tool_mode
                         FROM session_settings_revision_projection
                         WHERE session_id = ?1 AND settings_revision_id = ?2",
                        params![event.session_id.to_string(), *settings_revision_id as i64,],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, Option<String>>(1)?,
                                row.get::<_, String>(2)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(storage_error)?;
                if let Some(existing) = existing {
                    if existing != (connection_id.clone(), model_id.clone(), tool_mode.clone()) {
                        return Err(BelltowerError::InvalidState(format!(
                            "session settings revision {} is immutable",
                            settings_revision_id
                        )));
                    }
                } else {
                    tx.execute(
                        "INSERT INTO session_settings_revision_projection (
                            session_id, settings_revision_id, connection_id, model_id, tool_mode, updated_at
                        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![
                            event.session_id.to_string(),
                            *settings_revision_id as i64,
                            connection_id,
                            model_id,
                            tool_mode,
                            format_time(event.occurred_at)?,
                        ],
                    )
                    .map_err(storage_error)?;
                }
                if *settings_revision_id > current_settings_revision_id {
                    tx.execute(
                        "UPDATE sessions
                         SET connection_id = ?1, model_id = ?2, tool_mode = ?3,
                             settings_revision_id = ?4, updated_at = ?5
                         WHERE session_id = ?6",
                        params![
                            connection_id,
                            model_id,
                            tool_mode,
                            *settings_revision_id as i64,
                            format_time(event.occurred_at)?,
                            event.session_id.to_string(),
                        ],
                    )
                    .map_err(storage_error)?;
                }
            }
            EventPayload::SessionQueuedMessageEnqueued {
                message,
                settings_revision_id,
            } => {
                tx.execute(
                    "INSERT INTO queued_message_projection (queue_event_id, session_id, branch_id, message_json, settings_revision_id, status, source_seq, enqueued_at, resolved_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7, NULL)
                     ON CONFLICT(queue_event_id) DO UPDATE SET
                        message_json = excluded.message_json,
                        settings_revision_id = excluded.settings_revision_id,
                        status = 'pending',
                        source_seq = excluded.source_seq,
                        enqueued_at = excluded.enqueued_at,
                        resolved_at = NULL",
                    params![
                        event.event_id.to_string(),
                        event.session_id.to_string(),
                        event.branch_id.to_string(),
                        serde_json::to_string(message)?,
                        *settings_revision_id as i64,
                        seq_id,
                        format_time(event.occurred_at)?,
                    ],
                )
                .map_err(storage_error)?;
            }
            EventPayload::SessionQueuedMessageResolved {
                queue_event_id,
                outcome,
                ..
            } => {
                tx.execute(
                    "UPDATE queued_message_projection
                     SET status = ?1, resolved_at = ?2
                     WHERE queue_event_id = ?3",
                    params![
                        queued_message_resolution_status(outcome),
                        format_time(event.occurred_at)?,
                        queue_event_id.to_string(),
                    ],
                )
                .map_err(storage_error)?;
            }
            EventPayload::CompletionFinished { usage, cost, .. } => {
                Self::refresh_cost_summary_projection(
                    tx,
                    event.session_id,
                    event.occurred_at,
                    usage,
                    cost.as_ref(),
                )?;
            }
            EventPayload::BudgetConfigured { budget } => {
                Self::refresh_budget_projection_from_config(
                    tx,
                    event.session_id,
                    event.occurred_at,
                    budget,
                )?;
            }
            EventPayload::BudgetCheckpoint {
                tokens_used,
                turns_used,
                elapsed_seconds,
                cost_used_usd,
            } => {
                Self::refresh_budget_projection_from_checkpoint(
                    tx,
                    event.session_id,
                    event.occurred_at,
                    *tokens_used,
                    *turns_used,
                    *elapsed_seconds,
                    *cost_used_usd,
                )?;
            }
            EventPayload::SessionCancelled { .. } => {
                tx.execute(
                    "INSERT INTO session_control_projection (session_id, cancel_requested, updated_at)
                     VALUES (?1, 1, ?2)
                     ON CONFLICT(session_id) DO UPDATE SET cancel_requested = 1, updated_at = excluded.updated_at",
                    params![event.session_id.to_string(), format_time(event.occurred_at)?],
                )
                .map_err(storage_error)?;
            }
            EventPayload::SessionCancelCleared { .. } => {
                tx.execute(
                    "INSERT INTO session_control_projection (session_id, cancel_requested, updated_at)
                     VALUES (?1, 0, ?2)
                     ON CONFLICT(session_id) DO UPDATE SET cancel_requested = 0, updated_at = excluded.updated_at",
                    params![event.session_id.to_string(), format_time(event.occurred_at)?],
                )
                .map_err(storage_error)?;
            }
            EventPayload::SessionSteered {
                message,
                settings_revision_id,
            } => {
                tx.execute(
                    "INSERT INTO steer_projection (steer_event_id, session_id, branch_id, message, settings_revision_id, status, source_seq, enqueued_at, resolved_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7, NULL)
                     ON CONFLICT(steer_event_id) DO UPDATE SET
                        message = excluded.message,
                        settings_revision_id = excluded.settings_revision_id,
                        status = 'pending',
                        source_seq = excluded.source_seq,
                        enqueued_at = excluded.enqueued_at,
                        resolved_at = NULL",
                    params![
                        event.event_id.to_string(),
                        event.session_id.to_string(),
                        event.branch_id.to_string(),
                        message,
                        *settings_revision_id as i64,
                        seq_id,
                        format_time(event.occurred_at)?,
                    ],
                )
                .map_err(storage_error)?;
            }
            EventPayload::SessionSteersResolved {
                steer_event_ids,
                outcome,
                ..
            } => {
                for steer_event_id in steer_event_ids {
                    tx.execute(
                        "UPDATE steer_projection
                         SET status = ?1, resolved_at = ?2
                         WHERE steer_event_id = ?3",
                        params![
                            steer_resolution_status(outcome),
                            format_time(event.occurred_at)?,
                            steer_event_id.to_string(),
                        ],
                    )
                    .map_err(storage_error)?;
                }
            }
            EventPayload::BranchSummarized {
                branch_id, summary, ..
            } => {
                tx.execute(
                    "UPDATE branches SET summary = ?1 WHERE branch_id = ?2",
                    params![summary, branch_id.to_string()],
                )
                .map_err(storage_error)?;
            }
            EventPayload::SessionEnded { reason } => {
                tx.execute(
                    "UPDATE sessions SET status = ?1, updated_at = ?2 WHERE session_id = ?3",
                    params![
                        if reason.eq_ignore_ascii_case("completed") {
                            "completed"
                        } else {
                            "failed"
                        },
                        format_time(event.occurred_at)?,
                        event.session_id.to_string()
                    ],
                )
                .map_err(storage_error)?;
            }
            _ => {}
        }

        Ok(())
    }
}
