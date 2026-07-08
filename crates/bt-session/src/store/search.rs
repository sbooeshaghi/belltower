//! Canonical session-history search over the durable event log.
//!
//! This is intentionally exact-text search, not memory or retrieval. It returns
//! stable event/turn/tool references so callers can inspect the canonical record.

use super::{SqliteSessionStore, storage_error, to_sql_conversion};
use bt_core::{
    BelltowerError, BranchId, EventEnvelope, EventPayload, Message, MessagePart, Result, Role,
    SessionId, SessionSearchMatch, SessionSearchMatchKind, ToolCallId,
};
use rusqlite::params_from_iter;
use rusqlite::types::Value as SqlValue;
use serde_json::Value;

const SEARCH_EVENT_KINDS: &[&str] = &[
    "message.appended",
    "operator.command.recorded",
    "tool.call.requested",
    "tool.operation.recorded",
    "tool.approval.requested",
    "tool.approval.resolved",
    "tool.execution.finished",
    "session.error",
    "plan.updated",
    "branch.summarized",
    "context.compacted",
    "turn.context_manifest.recorded",
    "turn.instructions.recorded",
];
const DEFAULT_SNIPPET_RADIUS: usize = 96;

impl SqliteSessionStore {
    pub fn search_session_history(
        &self,
        session_id: SessionId,
        branch_id: Option<BranchId>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SessionSearchMatch>> {
        let needle = query.trim();
        if needle.is_empty() {
            return Err(BelltowerError::InvalidState(
                "session search query must not be empty".to_owned(),
            ));
        }
        let limit = limit.max(1);
        let events = self.load_search_events(session_id, branch_id)?;
        let mut matches = Vec::new();
        for event in events {
            collect_event_matches(&event, needle, &mut matches)?;
            if matches.len() >= limit {
                matches.truncate(limit);
                return Ok(matches);
            }
        }
        Ok(matches)
    }

    fn load_search_events(
        &self,
        session_id: SessionId,
        branch_id: Option<BranchId>,
    ) -> Result<Vec<EventEnvelope>> {
        if let Some(branch_id) = branch_id {
            self.load_branch_search_events(session_id, branch_id)
        } else {
            self.load_session_search_events(session_id)
        }
    }

    fn load_session_search_events(&self, session_id: SessionId) -> Result<Vec<EventEnvelope>> {
        let kind_placeholders = SEARCH_EVENT_KINDS
            .iter()
            .enumerate()
            .map(|(index, _)| format!("?{}", index + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT event_json, seq_id
             FROM events
             WHERE session_id = ?1 AND event_kind IN ({kind_placeholders})
             ORDER BY seq_id ASC"
        );
        let mut params = Vec::with_capacity(SEARCH_EVENT_KINDS.len() + 1);
        params.push(SqlValue::from(session_id.to_string()));
        for kind in SEARCH_EVENT_KINDS {
            params.push(SqlValue::from((*kind).to_owned()));
        }
        self.query_search_events(&sql, params)
    }

    fn load_branch_search_events(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
    ) -> Result<Vec<EventEnvelope>> {
        let lineage_bounds = self.branch_lineage_bounds(session_id, branch_id)?;
        let mut params =
            Vec::with_capacity(1 + (lineage_bounds.len() * 2) + SEARCH_EVENT_KINDS.len());
        params.push(SqlValue::from(session_id.to_string()));

        let mut clauses = Vec::with_capacity(lineage_bounds.len());
        for (branch_id, upper_bound) in lineage_bounds {
            params.push(SqlValue::from(branch_id.to_string()));
            let branch_idx = params.len();
            params.push(match upper_bound {
                Some(seq_id) => SqlValue::from(seq_id),
                None => SqlValue::Null,
            });
            let upper_idx = params.len();
            clauses.push(format!(
                "(e.branch_id = ?{branch_idx} AND (?{upper_idx} IS NULL OR e.seq_id <= ?{upper_idx}))"
            ));
        }

        let kind_start = params.len() + 1;
        for kind in SEARCH_EVENT_KINDS {
            params.push(SqlValue::from((*kind).to_owned()));
        }
        let kind_placeholders = (kind_start..kind_start + SEARCH_EVENT_KINDS.len())
            .map(|idx| format!("?{idx}"))
            .collect::<Vec<_>>()
            .join(", ");

        let sql = format!(
            "SELECT e.event_json, e.seq_id
             FROM events e
             WHERE e.session_id = ?1
               AND ({})
               AND e.event_kind IN ({kind_placeholders})
             ORDER BY e.seq_id ASC",
            clauses.join(" OR "),
        );

        let mut statement = self.connection.prepare(&sql).map_err(storage_error)?;
        let rows = statement
            .query_map(params_from_iter(params), parse_search_event)
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    fn query_search_events(&self, sql: &str, params: Vec<SqlValue>) -> Result<Vec<EventEnvelope>> {
        let mut statement = self.connection.prepare(sql).map_err(storage_error)?;
        let rows = statement
            .query_map(params_from_iter(params), parse_search_event)
            .map_err(storage_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }
}

fn parse_search_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventEnvelope> {
    let raw: String = row.get(0)?;
    let seq_id: i64 = row.get(1)?;
    let mut event: EventEnvelope = serde_json::from_str(&raw).map_err(to_sql_conversion)?;
    event.seq_id = Some(seq_id);
    Ok(event)
}

fn collect_event_matches(
    event: &EventEnvelope,
    query: &str,
    matches: &mut Vec<SessionSearchMatch>,
) -> Result<()> {
    match &event.payload {
        EventPayload::MessageAppended { message } => {
            collect_message_matches(event, message, query, matches)
        }
        EventPayload::OperatorCommandRecorded {
            command_type,
            raw_input,
            output,
            ..
        } => {
            push_if_matches(
                event,
                SessionSearchMatchKind::OperatorCommand,
                "raw_input",
                raw_input,
                query,
                SearchMetadata::default().command_type(command_type.clone()),
                matches,
            );
            push_if_matches(
                event,
                SessionSearchMatchKind::OperatorCommand,
                "output",
                output,
                query,
                SearchMetadata::default().command_type(command_type.clone()),
                matches,
            );
        }
        EventPayload::ToolCallRequested {
            call_id,
            tool_name,
            arguments,
        } => {
            push_if_matches(
                event,
                SessionSearchMatchKind::ToolCall,
                "tool_name",
                tool_name,
                query,
                SearchMetadata::default().tool(call_id.clone(), tool_name.clone()),
                matches,
            );
            push_json_if_matches(
                event,
                SessionSearchMatchKind::ToolCall,
                "arguments",
                arguments,
                query,
                SearchMetadata::default().tool(call_id.clone(), tool_name.clone()),
                matches,
            )?;
        }
        EventPayload::ToolOperationRecorded {
            call_id,
            tool_name,
            operation,
        } => {
            push_if_matches(
                event,
                SessionSearchMatchKind::ToolCall,
                "tool_name",
                tool_name,
                query,
                SearchMetadata::default().tool(call_id.clone(), tool_name.clone()),
                matches,
            );
            push_json_if_matches(
                event,
                SessionSearchMatchKind::ToolCall,
                "operation",
                &serde_json::to_value(operation)?,
                query,
                SearchMetadata::default().tool(call_id.clone(), tool_name.clone()),
                matches,
            )?;
        }
        EventPayload::ToolApprovalRequested {
            call_id,
            tool_name,
            snapshot,
        } => {
            push_if_matches(
                event,
                SessionSearchMatchKind::ToolCall,
                "tool_name",
                tool_name,
                query,
                SearchMetadata::default().tool(call_id.clone(), tool_name.clone()),
                matches,
            );
            push_json_if_matches(
                event,
                SessionSearchMatchKind::ToolCall,
                "approval_snapshot",
                &serde_json::to_value(snapshot)?,
                query,
                SearchMetadata::default().tool(call_id.clone(), tool_name.clone()),
                matches,
            )?;
        }
        EventPayload::ToolApprovalResolved {
            call_id,
            tool_name,
            request_fingerprint,
            resolution,
            decision,
        } => {
            push_if_matches(
                event,
                SessionSearchMatchKind::ToolCall,
                "tool_name",
                tool_name,
                query,
                SearchMetadata::default().tool(call_id.clone(), tool_name.clone()),
                matches,
            );
            push_json_if_matches(
                event,
                SessionSearchMatchKind::ToolCall,
                "approval",
                &serde_json::json!({
                    "request_fingerprint": request_fingerprint,
                    "resolution": resolution,
                    "decision": decision,
                }),
                query,
                SearchMetadata::default().tool(call_id.clone(), tool_name.clone()),
                matches,
            )?;
        }
        EventPayload::ToolExecutionFinished {
            call_id,
            tool_name,
            result,
        } => {
            push_if_matches(
                event,
                SessionSearchMatchKind::ToolResult,
                "tool_name",
                tool_name,
                query,
                SearchMetadata::default().tool(call_id.clone(), tool_name.clone()),
                matches,
            );
            push_json_if_matches(
                event,
                SessionSearchMatchKind::ToolResult,
                "result",
                &serde_json::to_value(result)?,
                query,
                SearchMetadata::default().tool(call_id.clone(), tool_name.clone()),
                matches,
            )?;
        }
        EventPayload::SessionError { code, message, .. } => {
            push_if_matches(
                event,
                SessionSearchMatchKind::SessionError,
                "code",
                code,
                query,
                SearchMetadata::default(),
                matches,
            );
            push_if_matches(
                event,
                SessionSearchMatchKind::SessionError,
                "message",
                message,
                query,
                SearchMetadata::default(),
                matches,
            );
        }
        EventPayload::PlanUpdated { items } => push_json_if_matches(
            event,
            SessionSearchMatchKind::Plan,
            "items",
            &serde_json::to_value(items)?,
            query,
            SearchMetadata::default(),
            matches,
        )?,
        EventPayload::BranchSummarized { summary, .. } => {
            push_if_matches(
                event,
                SessionSearchMatchKind::Context,
                "branch_summary",
                summary,
                query,
                SearchMetadata::default(),
                matches,
            );
        }
        EventPayload::ContextCompacted { summary, .. } => {
            push_if_matches(
                event,
                SessionSearchMatchKind::Context,
                "compaction_summary",
                summary,
                query,
                SearchMetadata::default(),
                matches,
            );
        }
        EventPayload::TurnContextManifestRecorded { manifest } => push_json_if_matches(
            event,
            SessionSearchMatchKind::Context,
            "context_manifest",
            &serde_json::to_value(manifest)?,
            query,
            SearchMetadata::default(),
            matches,
        )?,
        EventPayload::TurnInstructionProvenanceRecorded { provenance } => push_json_if_matches(
            event,
            SessionSearchMatchKind::Context,
            "instruction_provenance",
            &serde_json::to_value(provenance)?,
            query,
            SearchMetadata::default(),
            matches,
        )?,
        _ => {}
    }
    Ok(())
}

fn collect_message_matches(
    event: &EventEnvelope,
    message: &Message,
    query: &str,
    matches: &mut Vec<SessionSearchMatch>,
) {
    for (index, part) in message.parts.iter().enumerate() {
        match part {
            MessagePart::Text { text } => push_if_matches(
                event,
                SessionSearchMatchKind::Message,
                &format!("message.parts[{index}].text"),
                text,
                query,
                SearchMetadata::default().role(message.role.clone()),
                matches,
            ),
            MessagePart::Refusal {
                text: Some(text), ..
            } => push_if_matches(
                event,
                SessionSearchMatchKind::Message,
                &format!("message.parts[{index}].refusal"),
                text,
                query,
                SearchMetadata::default().role(message.role.clone()),
                matches,
            ),
            MessagePart::ToolCall { call } => {
                push_if_matches(
                    event,
                    SessionSearchMatchKind::ToolCall,
                    &format!("message.parts[{index}].tool_name"),
                    &call.tool_name,
                    query,
                    SearchMetadata::default().role(message.role.clone()).tool(
                        ToolCallId::new(call.call_id.clone()),
                        call.tool_name.clone(),
                    ),
                    matches,
                );
                if let Ok(arguments) = serde_json::to_string(&call.arguments) {
                    push_if_matches(
                        event,
                        SessionSearchMatchKind::ToolCall,
                        &format!("message.parts[{index}].arguments"),
                        &arguments,
                        query,
                        SearchMetadata::default().role(message.role.clone()).tool(
                            ToolCallId::new(call.call_id.clone()),
                            call.tool_name.clone(),
                        ),
                        matches,
                    );
                }
            }
            MessagePart::ToolResult { result } => {
                if let Ok(value) = serde_json::to_value(result)
                    && let Ok(rendered) = serde_json::to_string(&value)
                {
                    push_if_matches(
                        event,
                        SessionSearchMatchKind::ToolResult,
                        &format!("message.parts[{index}].result"),
                        &rendered,
                        query,
                        SearchMetadata::default()
                            .role(message.role.clone())
                            .tool(result.call_id.clone(), result.tool_name.clone()),
                        matches,
                    );
                }
            }
            MessagePart::Structured { value, .. } => {
                if let Ok(rendered) = serde_json::to_string(value) {
                    push_if_matches(
                        event,
                        SessionSearchMatchKind::Message,
                        &format!("message.parts[{index}].structured"),
                        &rendered,
                        query,
                        SearchMetadata::default().role(message.role.clone()),
                        matches,
                    );
                }
            }
            _ => {}
        }
    }
}

fn push_json_if_matches(
    event: &EventEnvelope,
    kind: SessionSearchMatchKind,
    field: &str,
    value: &Value,
    query: &str,
    metadata: SearchMetadata,
    matches: &mut Vec<SessionSearchMatch>,
) -> Result<()> {
    push_if_matches(
        event,
        kind,
        field,
        &serde_json::to_string(value)?,
        query,
        metadata,
        matches,
    );
    Ok(())
}

fn push_if_matches(
    event: &EventEnvelope,
    kind: SessionSearchMatchKind,
    field: &str,
    haystack: &str,
    query: &str,
    metadata: SearchMetadata,
    matches: &mut Vec<SessionSearchMatch>,
) {
    let Some(snippet) = snippet_for_match(haystack, query) else {
        return;
    };
    matches.push(SessionSearchMatch {
        session_id: event.session_id,
        branch_id: event.branch_id,
        turn_id: event.turn_id,
        seq_id: event.seq_id.unwrap_or_default(),
        event_id: event.event_id,
        occurred_at: event.occurred_at,
        kind,
        matched_field: field.to_owned(),
        snippet,
        role: metadata.role,
        tool_call_id: metadata.tool_call_id,
        tool_name: metadata.tool_name,
        command_type: metadata.command_type,
    });
}

fn snippet_for_match(haystack: &str, query: &str) -> Option<String> {
    let lower_haystack = haystack.to_lowercase();
    let lower_query = query.to_lowercase();
    let start = lower_haystack.find(&lower_query)?;
    let byte_start = floor_char_boundary(haystack, start.saturating_sub(DEFAULT_SNIPPET_RADIUS));
    let byte_end = ceil_char_boundary(
        haystack,
        (start + lower_query.len() + DEFAULT_SNIPPET_RADIUS).min(haystack.len()),
    );
    let prefix = if byte_start > 0 { "..." } else { "" };
    let suffix = if byte_end < haystack.len() { "..." } else { "" };
    let snippet = haystack[byte_start..byte_end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    Some(format!("{prefix}{snippet}{suffix}"))
}

fn floor_char_boundary(value: &str, mut index: usize) -> usize {
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(value: &str, mut index: usize) -> usize {
    while index < value.len() && !value.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[derive(Default)]
struct SearchMetadata {
    role: Option<Role>,
    tool_call_id: Option<ToolCallId>,
    tool_name: Option<String>,
    command_type: Option<String>,
}

impl SearchMetadata {
    fn role(mut self, role: Role) -> Self {
        self.role = Some(role);
        self
    }

    fn tool(mut self, call_id: ToolCallId, tool_name: String) -> Self {
        self.tool_call_id = Some(call_id);
        self.tool_name = Some(tool_name);
        self
    }

    fn command_type(mut self, command_type: String) -> Self {
        self.command_type = Some(command_type);
        self
    }
}
