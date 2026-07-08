//! Branch lineage and paged transcript loading helpers for the SQLite store.
//!
//! These helpers reconstruct branch-local and lineage-aware message/operator
//! windows from canonical event and projection tables. Append paths and
//! projection refresh logic live in sibling modules.

use super::{SqliteSessionStore, storage_error, to_sql_conversion};
use bt_core::{
    BelltowerError, BranchId, BranchRecord, EventEnvelope, Message, Result, Role, SessionId,
};
use rusqlite::params_from_iter;
use rusqlite::types::Value as SqlValue;
use rusqlite::{OptionalExtension, params};

use super::{ContextMessageRecord, RecordedOperatorCommandRecord, SequencedMessageRecord};

impl SqliteSessionStore {
    pub(super) fn load_session_messages_flat(&self, session_id: SessionId) -> Result<Vec<Message>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT content_json
             FROM message_projection
             WHERE session_id = ?1
             ORDER BY created_at ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![session_id.to_string()], |row| {
                let raw: String = row.get(0)?;
                serde_json::from_str(&raw).map_err(to_sql_conversion)
            })
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub(super) fn load_messages_for_branch(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
    ) -> Result<Vec<Message>> {
        let lineage = self.branch_lineage(session_id, branch_id)?;
        let mut messages = Vec::new();
        for index in 0..lineage.len() {
            let branch = &lineage[index];
            let upper_bound = lineage
                .get(index + 1)
                .and_then(|child| child.parent_event_id);
            messages.extend(self.load_messages_for_exact_branch(
                session_id,
                branch.branch_id,
                upper_bound,
            )?);
        }

        if let Some(summary) = lineage
            .last()
            .and_then(|branch| branch.summary.as_ref())
            .filter(|summary| !summary.trim().is_empty())
        {
            messages.insert(
                0,
                Message::text(Role::System, format!("Branch handoff summary:\n{summary}")),
            );
        }

        Ok(messages)
    }

    pub(super) fn load_context_messages_for_branch(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
    ) -> Result<Vec<ContextMessageRecord>> {
        let lineage = self.branch_lineage(session_id, branch_id)?;
        let mut messages = Vec::new();
        for index in 0..lineage.len() {
            let branch = &lineage[index];
            let upper_bound = lineage
                .get(index + 1)
                .and_then(|child| child.parent_event_id);
            messages.extend(self.load_context_messages_for_exact_branch(
                session_id,
                branch.branch_id,
                upper_bound,
            )?);
        }

        if let Some(summary) = lineage
            .last()
            .and_then(|branch| branch.summary.as_ref())
            .filter(|summary| !summary.trim().is_empty())
        {
            messages.insert(
                0,
                ContextMessageRecord {
                    message: Message::text(
                        Role::System,
                        format!("Branch handoff summary:\n{summary}"),
                    ),
                    source_branch_id: None,
                    source_seq_id: None,
                },
            );
        }

        Ok(messages)
    }

    pub(super) fn branch_lineage(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
    ) -> Result<Vec<BranchRecord>> {
        let mut lineage = Vec::new();
        let mut current = self
            .load_branch(session_id, branch_id)?
            .ok_or_else(|| BelltowerError::InvalidState("branch not found".to_owned()))?;
        lineage.push(current.clone());

        while let Some(parent_id) = current.parent_branch_id {
            current = self.load_branch(session_id, parent_id)?.ok_or_else(|| {
                BelltowerError::InvalidState("parent branch not found".to_owned())
            })?;
            lineage.push(current.clone());
        }

        lineage.reverse();
        Ok(lineage)
    }

    pub(super) fn load_messages_for_exact_branch(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        up_to_event_id: Option<bt_core::EventId>,
    ) -> Result<Vec<Message>> {
        let upper_bound = up_to_event_id.map(|id| id.to_string());
        let mut statement = self
            .connection
            .prepare(
                "SELECT mp.content_json
             FROM message_projection mp
             JOIN events e ON e.event_id = mp.event_id
             WHERE mp.session_id = ?1 AND mp.branch_id = ?2
               AND (?3 IS NULL OR e.seq_id <= (SELECT seq_id FROM events WHERE event_id = ?3))
             ORDER BY e.seq_id ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![session_id.to_string(), branch_id.to_string(), upper_bound],
                |row| {
                    let raw: String = row.get(0)?;
                    serde_json::from_str(&raw).map_err(to_sql_conversion)
                },
            )
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub(super) fn load_context_messages_for_exact_branch(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        up_to_event_id: Option<bt_core::EventId>,
    ) -> Result<Vec<ContextMessageRecord>> {
        let upper_bound = up_to_event_id.map(|id| id.to_string());
        let mut statement = self
            .connection
            .prepare(
                "SELECT mp.content_json, mp.branch_id, e.seq_id
             FROM message_projection mp
             JOIN events e ON e.event_id = mp.event_id
             WHERE mp.session_id = ?1 AND mp.branch_id = ?2
               AND (?3 IS NULL OR e.seq_id <= (SELECT seq_id FROM events WHERE event_id = ?3))
             ORDER BY e.seq_id ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![session_id.to_string(), branch_id.to_string(), upper_bound],
                |row| {
                    let raw: String = row.get(0)?;
                    let source_branch_id: String = row.get(1)?;
                    let source_seq_id: i64 = row.get(2)?;
                    Ok(ContextMessageRecord {
                        message: serde_json::from_str(&raw).map_err(to_sql_conversion)?,
                        source_branch_id: Some(
                            source_branch_id.parse().map_err(to_sql_conversion)?,
                        ),
                        source_seq_id: Some(source_seq_id),
                    })
                },
            )
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub(super) fn load_operator_command_events_for_exact_branch(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        up_to_event_id: Option<bt_core::EventId>,
    ) -> Result<Vec<EventEnvelope>> {
        let upper_bound = up_to_event_id.map(|id| id.to_string());
        let mut statement = self
            .connection
            .prepare(
                "SELECT e.event_json, e.seq_id
             FROM events e
             WHERE e.session_id = ?1 AND e.branch_id = ?2
               AND e.event_kind = 'operator.command.recorded'
               AND (?3 IS NULL OR e.seq_id <= (SELECT seq_id FROM events WHERE event_id = ?3))
             ORDER BY e.seq_id ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![session_id.to_string(), branch_id.to_string(), upper_bound],
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

    pub(super) fn branch_lineage_bounds(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
    ) -> Result<Vec<(BranchId, Option<i64>)>> {
        let lineage = self.branch_lineage(session_id, branch_id)?;
        let mut bounds = Vec::with_capacity(lineage.len());
        for index in 0..lineage.len() {
            let branch = &lineage[index];
            let upper_bound = lineage
                .get(index + 1)
                .and_then(|child| child.parent_event_id)
                .map(|event_id| self.event_seq_id(event_id))
                .transpose()?;
            bounds.push((branch.branch_id, upper_bound));
        }
        Ok(bounds)
    }

    fn event_seq_id(&self, event_id: bt_core::EventId) -> Result<i64> {
        self.connection
            .query_row(
                "SELECT seq_id FROM events WHERE event_id = ?1",
                params![event_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)?
            .ok_or_else(|| BelltowerError::InvalidState("event not found".to_owned()))
    }

    pub(super) fn load_page_rows<T, F>(
        &self,
        sql_template: &str,
        session_id: SessionId,
        lineage_bounds: &[(BranchId, Option<i64>)],
        before_seq_id: Option<i64>,
        limit: usize,
        mut map_row: F,
    ) -> Result<(Vec<T>, Option<i64>, Option<i64>, bool)>
    where
        T: SequencedRow,
        F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    {
        let mut params = Vec::with_capacity(2 + (lineage_bounds.len() * 2));
        params.push(SqlValue::from(session_id.to_string()));

        let mut clauses = Vec::with_capacity(lineage_bounds.len());
        for (branch_id, upper_bound) in lineage_bounds {
            params.push(SqlValue::from(branch_id.to_string()));
            let branch_idx = params.len();
            params.push(match upper_bound {
                Some(seq_id) => SqlValue::from(*seq_id),
                None => SqlValue::Null,
            });
            let upper_idx = params.len();
            clauses.push(format!(
                "(e.branch_id = ?{branch_idx} AND (?{upper_idx} IS NULL OR e.seq_id <= ?{upper_idx}))"
            ));
        }

        params.push(match before_seq_id {
            Some(seq_id) => SqlValue::from(seq_id),
            None => SqlValue::Null,
        });
        let before_idx = params.len();
        params.push(SqlValue::from((limit.saturating_add(1)) as i64));
        let limit_idx = params.len();

        let sql = sql_template
            .replace("{lineage_filter}", &format!("({})", clauses.join(" OR ")))
            .replace("{before_idx}", &before_idx.to_string())
            .replace("{limit_idx}", &limit_idx.to_string());

        let mut statement = self.connection.prepare(&sql).map_err(storage_error)?;
        let rows = statement
            .query_map(params_from_iter(params), |row| map_row(row))
            .map_err(storage_error)?;
        let mut rows = rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?;

        let has_more_before = rows.len() > limit;
        if has_more_before {
            rows.pop();
        }
        rows.reverse();

        let oldest_seq_id = first_seq_id(&rows);
        let newest_seq_id = last_seq_id(&rows);
        Ok((rows, oldest_seq_id, newest_seq_id, has_more_before))
    }
}

fn first_seq_id<T>(rows: &[T]) -> Option<i64>
where
    T: SequencedRow,
{
    rows.iter().find_map(SequencedRow::seq_id)
}

fn last_seq_id<T>(rows: &[T]) -> Option<i64>
where
    T: SequencedRow,
{
    rows.iter().rev().find_map(SequencedRow::seq_id)
}

pub(super) trait SequencedRow {
    fn seq_id(&self) -> Option<i64>;
}

impl SequencedRow for SequencedMessageRecord {
    fn seq_id(&self) -> Option<i64> {
        (self.seq_id > 0).then_some(self.seq_id)
    }
}

impl SequencedRow for RecordedOperatorCommandRecord {
    fn seq_id(&self) -> Option<i64> {
        Some(self.seq_id)
    }
}
