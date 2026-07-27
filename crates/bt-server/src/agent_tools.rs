//! Model-facing tools for durable parent-child session coordination.
//!
//! These executors adapt the canonical runtime/session graph to the agent tool
//! loop. They do not own lineage, mailbox, admission, or turn semantics.

use crate::AppState;
use bt_core::{
    ApprovalRequirement, BelltowerError, BranchId, ConnectionId, EventPayload,
    RelatedSessionDeliveryMode, RelatedSessionMessageDirection, RelatedSessionMessageId,
    RelatedSessionMessageKind, RelatedSessionMessageStatus, Result, Role, SessionId, ToolCallId,
    ToolContext, ToolDisplayGroup, ToolExecutionMode, ToolExecutor, ToolInterruptBehavior,
    ToolMetadata, ToolResultEnvelope, ToolRiskClass, ToolSpec,
};
use bt_runtime::{AdmittedTurn, BelltowerRuntime, TurnRunOutcome, TurnRunStopReason};
use bt_tools::BuiltInToolRegistry;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_WAIT_SECONDS: u64 = 5;
const MAX_WAIT_SECONDS: u64 = 30;
const MAX_RETURNED_MESSAGES: usize = 100;

type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<ToolResultEnvelope>> + Send + 'a>>;

pub(super) fn register_agent_tools(
    registry: &mut BuiltInToolRegistry,
    state: AppState,
    session_id: SessionId,
    branch_id: BranchId,
) {
    registry.register_arc(Arc::new(SpawnAgentTool {
        state: state.clone(),
        session_id,
        branch_id,
    }));
    registry.register_arc(Arc::new(SendAgentMessageTool {
        state: state.clone(),
        session_id,
        branch_id,
    }));
    registry.register_arc(Arc::new(ListAgentsTool {
        runtime: state.runtime.clone(),
        session_id,
    }));
    registry.register_arc(Arc::new(WaitAgentTool {
        runtime: state.runtime,
        session_id,
    }));
}

struct SpawnAgentTool {
    state: AppState,
    session_id: SessionId,
    branch_id: BranchId,
}

impl ToolExecutor for SpawnAgentTool {
    fn spec(&self) -> ToolSpec {
        // The registry is rebuilt per turn, so the spec can advertise the
        // LIVE connection inventory — models cannot guess ids like "ollama"
        // when the real id is "local".
        let mut connections = self
            .state
            .runtime
            .connections()
            .into_iter()
            .map(|connection| (connection.id.to_string(), connection.default_model))
            .collect::<Vec<_>>();
        connections.sort();
        let connection_ids = connections
            .iter()
            .map(|(id, _)| Value::String(id.clone()))
            .collect::<Vec<_>>();
        let inventory = connections
            .iter()
            .map(|(id, default_model)| format!("{id} (default model: {default_model})"))
            .collect::<Vec<_>>()
            .join("; ");
        ToolSpec {
            name: "spawn_agent".to_owned(),
            description: format!(
                "Spawn a child agent session for a bounded objective. The child may use a different configured connection and model. Children currently share the parent's project root, so allow_shared_workspace must be explicitly true and human approval is always required. Configured connections: {inventory}."
            ),
            parameters_schema: json!({
                "type": "object",
                "required": ["objective", "allow_shared_workspace", "call_id"],
                "properties": {
                    "call_id": {"type": "string"},
                    "objective": {
                        "type": "string",
                        "description": "A concrete, self-contained objective for the child agent."
                    },
                    "display_name": {"type": "string"},
                    "connection_id": {
                        "type": "string",
                        "enum": connection_ids,
                        "description": "Configured Belltower connection id. Omit to inherit the parent connection."
                    },
                    "model_id": {
                        "type": "string",
                        "description": "Model id for the child. Omit to inherit when using the parent connection."
                    },
                    "allow_shared_workspace": {
                        "type": "boolean",
                        "description": "Must be true to acknowledge that parent and child currently share one project root."
                    },
                    "approval_mode": {
                        "type": "string",
                        "enum": ["inherit", "auto", "prompt"],
                        "description": "Child tool-approval policy. inherit (default) copies the parent session's mode; auto resolves every child approval as a recorded policy decision; prompt requires human approval."
                    }
                },
                "additionalProperties": false
            }),
            metadata: ToolMetadata {
                risk_class: ToolRiskClass::High,
                is_read_only: false,
                is_concurrency_safe: false,
                interrupt_behavior: ToolInterruptBehavior::Immediate,
                execution_mode: ToolExecutionMode::Immediate,
                should_defer: false,
                catalogue_tags: vec![
                    "agent".to_owned(),
                    "subagent".to_owned(),
                    "delegate".to_owned(),
                    "workflow".to_owned(),
                ],
                display_group: ToolDisplayGroup::Workflow,
            },
        }
    }

    fn approval_requirement(&self, _arguments: &Value) -> ApprovalRequirement {
        ApprovalRequirement::Always
    }

    fn execute(&self, arguments: Value, _context: ToolContext) -> ToolFuture<'_> {
        Box::pin(async move {
            let call_id = ToolCallId::new(required_string(&arguments, "call_id")?);
            if !arguments
                .get("allow_shared_workspace")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return Err(BelltowerError::Tool(
                    "spawn_agent requires allow_shared_workspace=true because child sessions currently share the parent's project root"
                        .to_owned(),
                ));
            }
            let objective = required_string(&arguments, "objective")?;
            let display_name = optional_string(&arguments, "display_name");
            let connection_id = optional_string(&arguments, "connection_id").map(ConnectionId::new);
            let model_id = optional_string(&arguments, "model_id");
            let parent_turn_id = Some(active_turn_id(
                &self.state.runtime,
                self.session_id,
                self.branch_id,
            )?);
            let child_auto_approval = match optional_string(&arguments, "approval_mode")
                .as_deref()
                .unwrap_or("inherit")
            {
                "inherit" => self.state.runtime.session_approval_is_auto(self.session_id),
                "auto" => true,
                "prompt" => false,
                value => {
                    return Err(BelltowerError::Tool(format!(
                        "unsupported approval_mode `{value}`; expected inherit, auto, or prompt"
                    )));
                }
            };
            let (child, child_branch, receipt) = self
                .state
                .runtime
                .spawn_child_session_with_initial_objective(
                    self.session_id,
                    self.branch_id,
                    parent_turn_id,
                    objective,
                    display_name,
                    connection_id,
                    model_id,
                )?;
            if child_auto_approval {
                self.state
                    .runtime
                    .set_session_approval_auto(child.session_id, true);
                self.state.runtime.record_operator_command(
                    child.session_id,
                    child_branch.branch_id,
                    "approval_mode".to_owned(),
                    "approval_mode auto (from spawn)".to_owned(),
                    "Child session approvals resolve automatically as recorded policy decisions."
                        .to_owned(),
                    true,
                )?;
            }
            let admitted = self
                .state
                .runtime
                .claim_next_related_session_message(child.session_id)?
                .ok_or_else(|| {
                    BelltowerError::InvalidState(
                        "new child session could not claim its initial objective".to_owned(),
                    )
                })?;
            dispatch_agent_turn(self.state.clone(), admitted)?;

            Ok(ToolResultEnvelope {
                call_id,
                tool_name: "spawn_agent".to_owned(),
                is_error: false,
                output: json!({
                    "child_session_id": child.session_id,
                    "child_branch_id": child_branch.branch_id,
                    "connection_id": child.connection_id,
                    "model_id": child.model_id,
                    "objective": child.objective,
                    "message_id": receipt.message_id,
                    "wait_after_seq_id": receipt.sent_seq_id,
                    "status": "running"
                }),
                duration_ms: None,
            })
        })
    }
}

struct SendAgentMessageTool {
    state: AppState,
    session_id: SessionId,
    branch_id: BranchId,
}

impl ToolExecutor for SendAgentMessageTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "send_agent_message".to_owned(),
            description: "Send a durable typed message to any agent in this session tree (parent, child, or sibling). notify records context for a future turn; wake also requests an idle destination turn without interrupting active work. Parents are not copied on peer messages; use list_agents to inspect a related session's mailbox.".to_owned(),
            parameters_schema: json!({
                "type": "object",
                "required": ["target_session_id", "text", "call_id"],
                "properties": {
                    "call_id": {"type": "string"},
                    "target_session_id": {"type": "string"},
                    "text": {"type": "string"},
                    "kind": {
                        "type": "string",
                        "enum": ["instruction", "question", "answer", "progress", "result", "error"]
                    },
                    "delivery_mode": {
                        "type": "string",
                        "enum": ["notify", "wake"]
                    },
                    "in_reply_to": {"type": "string"}
                },
                "additionalProperties": false
            }),
            metadata: ToolMetadata {
                risk_class: ToolRiskClass::Moderate,
                is_read_only: false,
                is_concurrency_safe: true,
                interrupt_behavior: ToolInterruptBehavior::Immediate,
                execution_mode: ToolExecutionMode::Immediate,
                should_defer: false,
                catalogue_tags: vec![
                    "agent".to_owned(),
                    "subagent".to_owned(),
                    "message".to_owned(),
                    "workflow".to_owned(),
                ],
                display_group: ToolDisplayGroup::Workflow,
            },
        }
    }

    fn approval_requirement(&self, _arguments: &Value) -> ApprovalRequirement {
        ApprovalRequirement::Never
    }

    fn execute(&self, arguments: Value, _context: ToolContext) -> ToolFuture<'_> {
        Box::pin(async move {
            let call_id = ToolCallId::new(required_string(&arguments, "call_id")?);
            let target_session_id = parse_session_id(&arguments, "target_session_id")?;
            ensure_same_lineage(&self.state.runtime, self.session_id, target_session_id)?;
            let target_branch = self
                .state
                .runtime
                .default_branch(target_session_id)?
                .ok_or_else(|| BelltowerError::NotFound("target session branch".to_owned()))?;
            let kind = parse_message_kind(optional_string(&arguments, "kind").as_deref())?;
            let delivery_mode =
                parse_delivery_mode(optional_string(&arguments, "delivery_mode").as_deref())?;
            let in_reply_to = optional_string(&arguments, "in_reply_to")
                .map(|value| parse_related_message_id(&value))
                .transpose()?;
            let caused_by_turn_id = Some(active_turn_id(
                &self.state.runtime,
                self.session_id,
                self.branch_id,
            )?);
            let receipt = self.state.runtime.send_related_session_message(
                self.session_id,
                self.branch_id,
                caused_by_turn_id,
                target_session_id,
                target_branch.branch_id,
                kind,
                delivery_mode,
                in_reply_to,
                required_string(&arguments, "text")?,
                Vec::new(),
            )?;
            let mut dispatched = false;
            if delivery_mode == RelatedSessionDeliveryMode::Wake
                && let Some(admitted) = self
                    .state
                    .runtime
                    .claim_next_related_session_message(target_session_id)?
            {
                dispatch_agent_turn(self.state.clone(), admitted)?;
                dispatched = true;
            }
            Ok(ToolResultEnvelope {
                call_id,
                tool_name: "send_agent_message".to_owned(),
                is_error: false,
                output: json!({
                    "target_session_id": target_session_id,
                    "target_branch_id": target_branch.branch_id,
                    "message_id": receipt.message_id,
                    "status": receipt.status,
                    "dispatched": dispatched
                }),
                duration_ms: None,
            })
        })
    }
}

struct ListAgentsTool {
    runtime: Arc<BelltowerRuntime>,
    session_id: SessionId,
}

impl ToolExecutor for ListAgentsTool {
    fn spec(&self) -> ToolSpec {
        read_tool_spec(
            "list_agents",
            "Inspect the canonical session lineage, runtime states, and durable messages visible from the current agent. Pass target_session_id to read the mailbox of another session in this tree instead of your own — peer traffic is never pushed to you, but any of it can be inspected here on demand.",
            json!({
                "type": "object",
                "required": ["call_id"],
                "properties": {
                    "call_id": {"type": "string"},
                    "target_session_id": {
                        "type": "string",
                        "description": "Optional session in this lineage tree whose mailbox to return instead of the caller's."
                    }
                },
                "additionalProperties": false
            }),
        )
    }

    fn approval_requirement(&self, _arguments: &Value) -> ApprovalRequirement {
        ApprovalRequirement::Never
    }

    fn execute(&self, arguments: Value, _context: ToolContext) -> ToolFuture<'_> {
        Box::pin(async move {
            let call_id = ToolCallId::new(required_string(&arguments, "call_id")?);
            let mailbox_session_id = match optional_string(&arguments, "target_session_id") {
                Some(value) => {
                    let target = parse_session_id_value(&value)?;
                    ensure_same_lineage(&self.runtime, self.session_id, target)?;
                    target
                }
                None => self.session_id,
            };
            let workflow = self
                .runtime
                .session_workflow(self.session_id)?
                .ok_or_else(|| BelltowerError::NotFound("session workflow".to_owned()))?;
            let messages = latest_messages(
                self.runtime.related_session_messages(mailbox_session_id)?,
                MAX_RETURNED_MESSAGES,
            );
            Ok(ToolResultEnvelope {
                call_id,
                tool_name: "list_agents".to_owned(),
                is_error: false,
                output: json!({
                    "workflow": workflow,
                    "mailbox_session_id": mailbox_session_id,
                    "messages": messages
                }),
                duration_ms: None,
            })
        })
    }
}

struct WaitAgentTool {
    runtime: Arc<BelltowerRuntime>,
    session_id: SessionId,
}

impl ToolExecutor for WaitAgentTool {
    fn spec(&self) -> ToolSpec {
        read_tool_spec(
            "wait_agent",
            "Wait for a bounded interval for a related session in this tree to send a durable message. This observes activity; it does not join or cancel the other agent.",
            json!({
                "type": "object",
                "required": ["call_id"],
                "properties": {
                    "call_id": {"type": "string"},
                    "target_session_id": {"type": "string"},
                    "after_seq_id": {"type": "integer", "minimum": 0},
                    "timeout_seconds": {"type": "integer", "minimum": 1, "maximum": MAX_WAIT_SECONDS}
                },
                "additionalProperties": false
            }),
        )
    }

    fn approval_requirement(&self, _arguments: &Value) -> ApprovalRequirement {
        ApprovalRequirement::Never
    }

    fn execute(&self, arguments: Value, _context: ToolContext) -> ToolFuture<'_> {
        Box::pin(async move {
            let call_id = ToolCallId::new(required_string(&arguments, "call_id")?);
            let target_session_id = optional_string(&arguments, "target_session_id")
                .map(|value| parse_session_id_value(&value))
                .transpose()?;
            if let Some(target_session_id) = target_session_id {
                ensure_same_lineage(&self.runtime, self.session_id, target_session_id)?;
            }
            let after_seq_id = arguments
                .get("after_seq_id")
                .and_then(Value::as_i64)
                .unwrap_or(0)
                .max(0);
            let timeout_seconds = arguments
                .get("timeout_seconds")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_WAIT_SECONDS)
                .clamp(1, MAX_WAIT_SECONDS);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_seconds);
            loop {
                let messages = matching_received_messages(
                    &self.runtime,
                    self.session_id,
                    target_session_id,
                    after_seq_id,
                )?;
                if !messages.is_empty() || tokio::time::Instant::now() >= deadline {
                    let workflow = self
                        .runtime
                        .session_workflow(self.session_id)?
                        .ok_or_else(|| BelltowerError::NotFound("session workflow".to_owned()))?;
                    return Ok(ToolResultEnvelope {
                        call_id,
                        tool_name: "wait_agent".to_owned(),
                        is_error: false,
                        output: json!({
                            "timed_out": messages.is_empty(),
                            "messages": messages,
                            "workflow": workflow
                        }),
                        duration_ms: None,
                    });
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
    }
}

fn dispatch_agent_turn(state: AppState, admitted: AdmittedTurn) -> Result<()> {
    let session = state
        .runtime
        .load_session(admitted.session_id())?
        .ok_or_else(|| BelltowerError::NotFound("agent session".to_owned()))?;
    let branch = state
        .runtime
        .load_branch(admitted.session_id(), admitted.branch_id())?
        .ok_or_else(|| BelltowerError::NotFound("agent branch".to_owned()))?;
    crate::detach_session_turn(&state, session, branch, admitted);
    Ok(())
}

/// Background pump that dispatches Wake-mode related-session messages to
/// idle destinations. Tools claim eagerly at the send site; this pump is the
/// single mechanism that catches everything else (settled child results,
/// wakes raced against a busy destination whose turn ended before post-turn
/// controls saw the message, recovery gaps). Claiming is atomic, so a
/// duplicate trigger resolves to `None` instead of a double dispatch.
pub(super) fn spawn_wake_pump(state: AppState) {
    let mut events = state.runtime.subscribe();
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => {
                    let EventPayload::RelatedSessionMessageRecorded {
                        direction, message, ..
                    } = &event.payload
                    else {
                        continue;
                    };
                    if *direction != RelatedSessionMessageDirection::Received
                        || message.delivery_mode != RelatedSessionDeliveryMode::Wake
                    {
                        continue;
                    }
                    dispatch_pending_wakes(&state, event.session_id);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "wake pump lagged; sweeping pending agent turns");
                    if let Err(error) = recover_pending_agent_turns(state.clone()) {
                        tracing::warn!(error = %error, "wake pump sweep failed");
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn dispatch_pending_wakes(state: &AppState, destination_session_id: SessionId) {
    match state
        .runtime
        .claim_next_related_session_message(destination_session_id)
    {
        Ok(Some(admitted)) => {
            if let Err(error) = dispatch_agent_turn(state.clone(), admitted) {
                tracing::warn!(
                    session_id = %destination_session_id,
                    error = %error,
                    "failed to dispatch woken agent turn"
                );
            }
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(
                session_id = %destination_session_id,
                error = %error,
                "failed to claim wake message"
            );
        }
    }
}

pub(super) fn recover_pending_agent_turns(state: AppState) -> Result<usize> {
    let pending = state.runtime.all_pending_related_session_messages()?;
    let mut visited_sessions = HashSet::new();
    let mut dispatched = 0usize;
    for record in pending {
        let destination_session_id = record.message.destination_session_id;
        if !visited_sessions.insert(destination_session_id) {
            continue;
        }
        let Some(admitted) = state
            .runtime
            .claim_next_related_session_message(destination_session_id)?
        else {
            continue;
        };
        dispatch_agent_turn(state.clone(), admitted)?;
        dispatched = dispatched.saturating_add(1);
    }
    Ok(dispatched)
}

/// Settles this child session's reply obligations after a turn run: every
/// claimed Instruction/Question from a related session that has not yet
/// received a terminal reply gets one derived from the outcome. Runs at the
/// `run_session_turn` choke point, so approval- and input-resumed child turns
/// report back exactly like dispatch-path turns. Terminal replies are sent
/// with Wake delivery: an idle parent runs a turn to consume the result
/// instead of holding it forever. Waking cannot ping-pong — a woken turn
/// claims a Result/Error message, which never creates a reply obligation.
pub(super) fn settle_child_reply_obligations(
    state: &AppState,
    session: &bt_core::SessionRecord,
    branch: &bt_core::BranchRecord,
    outcome: &std::result::Result<TurnRunOutcome, crate::ApiError>,
) -> Result<()> {
    if session.parent_session_id.is_none() {
        return Ok(());
    }
    let records = state.runtime.related_session_messages(session.session_id)?;
    let (kind, text) = match outcome {
        Ok(outcome) => match outcome.stop_reason {
            TurnRunStopReason::Complete => (
                RelatedSessionMessageKind::Result,
                latest_assistant_text(&state.runtime, session.session_id, branch.branch_id)?
                    .unwrap_or_else(|| "Subagent turn completed.".to_owned()),
            ),
            TurnRunStopReason::AwaitingApproval => (
                RelatedSessionMessageKind::Progress,
                "Subagent is waiting for tool approval.".to_owned(),
            ),
            TurnRunStopReason::AwaitingInput => (
                RelatedSessionMessageKind::Progress,
                "Subagent is waiting for operator input.".to_owned(),
            ),
            TurnRunStopReason::BudgetExhausted => (
                RelatedSessionMessageKind::Error,
                "Subagent stopped because its budget was exhausted.".to_owned(),
            ),
            TurnRunStopReason::Cancelled => (
                RelatedSessionMessageKind::Error,
                "Subagent turn was cancelled.".to_owned(),
            ),
        },
        Err(error) => (
            RelatedSessionMessageKind::Error,
            format!("Subagent turn failed: {}", error.0),
        ),
    };
    let terminal = matches!(
        kind,
        RelatedSessionMessageKind::Result | RelatedSessionMessageKind::Error
    );

    for record in records.iter().filter(|record| {
        record.direction == RelatedSessionMessageDirection::Received
            && record.status == RelatedSessionMessageStatus::Claimed
            && matches!(
                record.message.kind,
                RelatedSessionMessageKind::Instruction | RelatedSessionMessageKind::Question
            )
    }) {
        let message_id = record.message.message_id;
        let already_terminally_replied = records.iter().any(|candidate| {
            candidate.direction == RelatedSessionMessageDirection::Sent
                && candidate.message.in_reply_to == Some(message_id)
                && matches!(
                    candidate.message.kind,
                    RelatedSessionMessageKind::Result | RelatedSessionMessageKind::Error
                )
        });
        if already_terminally_replied {
            continue;
        }
        let duplicate_progress = !terminal
            && records.iter().any(|candidate| {
                candidate.direction == RelatedSessionMessageDirection::Sent
                    && candidate.message.in_reply_to == Some(message_id)
                    && candidate.message.text == text
            });
        if duplicate_progress {
            continue;
        }
        state.runtime.send_related_session_message(
            session.session_id,
            branch.branch_id,
            None,
            record.message.source_session_id,
            record.message.source_branch_id,
            kind,
            if terminal {
                RelatedSessionDeliveryMode::Wake
            } else {
                RelatedSessionDeliveryMode::Notify
            },
            Some(message_id),
            text.clone(),
            Vec::new(),
        )?;
    }
    Ok(())
}

fn latest_assistant_text(
    runtime: &BelltowerRuntime,
    session_id: SessionId,
    branch_id: BranchId,
) -> Result<Option<String>> {
    Ok(runtime
        .messages(session_id, Some(branch_id))?
        .into_iter()
        .rev()
        .find(|message| message.role == Role::Assistant)
        .map(|message| message.text_parts().collect::<Vec<_>>().join("\n"))
        .filter(|text| !text.trim().is_empty()))
}

fn matching_received_messages(
    runtime: &BelltowerRuntime,
    session_id: SessionId,
    target_session_id: Option<SessionId>,
    after_seq_id: i64,
) -> Result<Vec<bt_core::RelatedSessionMessageRecord>> {
    Ok(latest_messages(
        runtime
            .related_session_messages(session_id)?
            .into_iter()
            .filter(|record| {
                record.direction == RelatedSessionMessageDirection::Received
                    && record.seq_id > after_seq_id
                    && target_session_id
                        .is_none_or(|target| record.message.source_session_id == target)
            })
            .collect(),
        MAX_RETURNED_MESSAGES,
    ))
}

fn latest_messages(
    mut messages: Vec<bt_core::RelatedSessionMessageRecord>,
    limit: usize,
) -> Vec<bt_core::RelatedSessionMessageRecord> {
    if messages.len() > limit {
        messages.drain(..messages.len() - limit);
    }
    messages
}

/// Agent messaging and mailbox inspection are scoped to one session tree:
/// any two sessions sharing a lineage root may interact (parent-child,
/// siblings, cousins), and nothing crosses trees. Parents are deliberately
/// NOT copied on peer traffic — every message is already durable in the
/// event log, and a coordinator inspects on demand via `list_agents`.
pub(super) fn ensure_same_lineage(
    runtime: &BelltowerRuntime,
    source_session_id: SessionId,
    target_session_id: SessionId,
) -> Result<()> {
    if source_session_id == target_session_id {
        return Err(BelltowerError::Protocol(
            "agent messaging requires a related session other than the caller".to_owned(),
        ));
    }
    let source_root = lineage_root(runtime, source_session_id)?;
    let target_root = lineage_root(runtime, target_session_id)?;
    if source_root == target_root {
        Ok(())
    } else {
        Err(BelltowerError::Protocol(
            "agent messaging is limited to sessions in the same lineage tree".to_owned(),
        ))
    }
}

fn lineage_root(runtime: &BelltowerRuntime, session_id: SessionId) -> Result<SessionId> {
    let mut visited = HashSet::new();
    let mut cursor = runtime
        .load_session(session_id)?
        .ok_or_else(|| BelltowerError::NotFound(format!("session `{session_id}`")))?;
    visited.insert(cursor.session_id);
    while let Some(parent_id) = cursor.parent_session_id {
        if !visited.insert(parent_id) {
            return Err(BelltowerError::InvalidState(
                "session lineage contains a cycle".to_owned(),
            ));
        }
        cursor = runtime.load_session(parent_id)?.ok_or_else(|| {
            BelltowerError::InvalidState(format!(
                "session lineage references missing parent `{parent_id}`"
            ))
        })?;
    }
    Ok(cursor.session_id)
}

fn read_tool_spec(name: &str, description: &str, parameters_schema: Value) -> ToolSpec {
    ToolSpec {
        name: name.to_owned(),
        description: description.to_owned(),
        parameters_schema,
        metadata: ToolMetadata {
            risk_class: ToolRiskClass::Safe,
            is_read_only: true,
            is_concurrency_safe: true,
            interrupt_behavior: ToolInterruptBehavior::Immediate,
            execution_mode: ToolExecutionMode::Immediate,
            should_defer: false,
            catalogue_tags: vec![
                "agent".to_owned(),
                "subagent".to_owned(),
                "workflow".to_owned(),
                "inspection".to_owned(),
            ],
            display_group: ToolDisplayGroup::Workflow,
        },
    }
}

fn required_string(arguments: &Value, field: &str) -> Result<String> {
    optional_string(arguments, field)
        .ok_or_else(|| BelltowerError::Tool(format!("missing string argument `{field}`")))
}

fn optional_string(arguments: &Value, field: &str) -> Option<String> {
    arguments
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_session_id(arguments: &Value, field: &str) -> Result<SessionId> {
    parse_session_id_value(&required_string(arguments, field)?)
}

fn parse_session_id_value(value: &str) -> Result<SessionId> {
    SessionId::from_str(value)
        .map_err(|error| BelltowerError::Tool(format!("invalid session id `{value}`: {error}")))
}

fn parse_related_message_id(value: &str) -> Result<RelatedSessionMessageId> {
    RelatedSessionMessageId::from_str(value).map_err(|error| {
        BelltowerError::Tool(format!(
            "invalid related-session message id `{value}`: {error}"
        ))
    })
}

fn active_turn_id(
    runtime: &BelltowerRuntime,
    session_id: SessionId,
    branch_id: BranchId,
) -> Result<bt_core::TurnId> {
    runtime
        .active_turn_claim(session_id)?
        .filter(|(claim_branch_id, _)| *claim_branch_id == branch_id)
        .map(|(_, turn_id)| turn_id)
        .ok_or_else(|| {
            BelltowerError::InvalidState(
                "model-facing agent tools require an active caller turn".to_owned(),
            )
        })
}

fn parse_message_kind(raw: Option<&str>) -> Result<RelatedSessionMessageKind> {
    match raw.unwrap_or("progress") {
        "instruction" => Ok(RelatedSessionMessageKind::Instruction),
        "question" => Ok(RelatedSessionMessageKind::Question),
        "answer" => Ok(RelatedSessionMessageKind::Answer),
        "progress" => Ok(RelatedSessionMessageKind::Progress),
        "result" => Ok(RelatedSessionMessageKind::Result),
        "error" => Ok(RelatedSessionMessageKind::Error),
        value => Err(BelltowerError::Tool(format!(
            "unsupported related-session message kind `{value}`"
        ))),
    }
}

fn parse_delivery_mode(raw: Option<&str>) -> Result<RelatedSessionDeliveryMode> {
    match raw.unwrap_or("notify") {
        "notify" => Ok(RelatedSessionDeliveryMode::Notify),
        "wake" => Ok(RelatedSessionDeliveryMode::Wake),
        value => Err(BelltowerError::Tool(format!(
            "unsupported related-session delivery mode `{value}`"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_core::{BelltowerConfig, RelatedSessionMessageStatus, SessionToolMode};
    use tempfile::NamedTempFile;

    fn context() -> ToolContext {
        ToolContext {
            project_root: "/tmp/project".into(),
        }
    }

    fn fixture() -> (
        AppState,
        bt_core::SessionRecord,
        bt_core::BranchRecord,
        NamedTempFile,
    ) {
        let config = BelltowerConfig::from_embedded().expect("config");
        let file = NamedTempFile::new().expect("tempfile");
        let runtime = Arc::new(BelltowerRuntime::open(config, file.path()).expect("runtime"));
        let (session, branch) = runtime
            .create_session(
                "/tmp/project".into(),
                ConnectionId::new("local"),
                None,
                SessionToolMode::Extended,
                Some("coordinator".to_owned()),
                None,
            )
            .expect("session");
        (
            AppState {
                runtime,
                token: Arc::new("test-token".to_owned()),
            },
            session,
            branch,
            file,
        )
    }

    fn registry(
        state: AppState,
        session_id: SessionId,
        branch_id: BranchId,
    ) -> BuiltInToolRegistry {
        let mut registry = BuiltInToolRegistry::default();
        register_agent_tools(&mut registry, state, session_id, branch_id);
        registry
    }

    #[tokio::test]
    async fn agent_tool_contract_requires_approval_and_shared_workspace_consent() {
        let (state, session, branch, _file) = fixture();
        let registry = registry(state, session.session_id, branch.branch_id);
        let names = registry
            .descriptors()
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "list_agents",
                "send_agent_message",
                "spawn_agent",
                "wait_agent"
            ]
        );

        let spawn = registry.get("spawn_agent").expect("spawn tool");
        assert_eq!(
            spawn.approval_requirement(&json!({})),
            ApprovalRequirement::Always
        );
        assert_eq!(spawn.spec().metadata.risk_class, ToolRiskClass::High);
        let error = spawn
            .execute(
                json!({
                    "call_id": "call-spawn",
                    "objective": "prove the theorem",
                    "allow_shared_workspace": false
                }),
                context(),
            )
            .await
            .expect_err("shared workspace consent is required");
        assert!(error.to_string().contains("allow_shared_workspace=true"));

        assert_eq!(
            registry
                .get("send_agent_message")
                .expect("send tool")
                .approval_requirement(&json!({})),
            ApprovalRequirement::Never
        );
    }

    #[tokio::test]
    async fn spawn_agent_uses_the_requested_child_connection_and_model() {
        let (state, parent, parent_branch, _file) = fixture();
        let admitted = state
            .runtime
            .admit_user_message(
                &parent,
                &parent_branch,
                bt_core::Message::text(Role::User, "delegate the independent review"),
            )
            .expect("admit parent turn");
        assert!(matches!(
            admitted,
            bt_runtime::UserMessageAdmission::Started(_)
        ));

        let result = registry(state.clone(), parent.session_id, parent_branch.branch_id)
            .get("spawn_agent")
            .expect("spawn agent tool")
            .execute(
                json!({
                    "call_id": "call-mixed-model-spawn",
                    "objective": "review the proof with a different model",
                    "display_name": "independent-reviewer",
                    "connection_id": "openai",
                    "model_id": "gpt-5.4-mini",
                    "allow_shared_workspace": true
                }),
                context(),
            )
            .await
            .expect("spawn mixed-model child");

        let child_session_id = parse_session_id(&result.output, "child_session_id")
            .expect("parse spawned child session id");
        let child = state
            .runtime
            .load_session(child_session_id)
            .expect("load child session")
            .expect("spawned child");
        assert_eq!(child.connection_id, ConnectionId::new("openai"));
        assert_eq!(child.model_id.as_deref(), Some("gpt-5.4-mini"));
        assert_eq!(result.output["status"], "running");
    }

    #[tokio::test]
    async fn parent_and_child_exchange_durable_messages_through_model_tools() {
        let (state, parent, parent_branch, _file) = fixture();
        let (child, child_branch) = state
            .runtime
            .spawn_child_session(
                parent.session_id,
                parent_branch.branch_id,
                None,
                "seek a counterexample".to_owned(),
                Some("critic".to_owned()),
                Some(ConnectionId::new("local")),
                Some("qwen3.5:latest".to_owned()),
            )
            .expect("child session");
        let parent_tools = registry(state.clone(), parent.session_id, parent_branch.branch_id);
        let child_tools = registry(state.clone(), child.session_id, child_branch.branch_id);
        let parent_admission = state
            .runtime
            .admit_user_message(
                &parent,
                &parent_branch,
                bt_core::Message::text(Role::User, "coordinate the search"),
            )
            .expect("admit parent turn");
        assert!(matches!(
            parent_admission,
            bt_runtime::UserMessageAdmission::Started(_)
        ));
        let child_admission = state
            .runtime
            .admit_user_message(
                &child,
                &child_branch,
                bt_core::Message::text(Role::User, "investigate counterexamples"),
            )
            .expect("admit child turn");
        assert!(matches!(
            child_admission,
            bt_runtime::UserMessageAdmission::Started(_)
        ));

        let question = parent_tools
            .get("send_agent_message")
            .expect("parent send tool")
            .execute(
                json!({
                    "call_id": "call-question",
                    "target_session_id": child.session_id,
                    "kind": "question",
                    "delivery_mode": "notify",
                    "text": "What is the smallest counterexample?"
                }),
                context(),
            )
            .await
            .expect("send question");
        let question_id = question.output["message_id"]
            .as_str()
            .expect("message id")
            .to_owned();

        child_tools
            .get("send_agent_message")
            .expect("child send tool")
            .execute(
                json!({
                    "call_id": "call-answer",
                    "target_session_id": parent.session_id,
                    "kind": "answer",
                    "delivery_mode": "notify",
                    "in_reply_to": question_id,
                    "text": "No counterexample exists below 100."
                }),
                context(),
            )
            .await
            .expect("send answer");

        let waited = parent_tools
            .get("wait_agent")
            .expect("wait tool")
            .execute(
                json!({
                    "call_id": "call-wait",
                    "target_session_id": child.session_id,
                    "after_seq_id": 0,
                    "timeout_seconds": 1
                }),
                context(),
            )
            .await
            .expect("wait for answer");
        assert_eq!(waited.output["timed_out"], false);
        assert!(
            waited.output["messages"]
                .as_array()
                .is_some_and(|messages| {
                    messages.iter().any(|message| {
                        message["message"]["text"] == "No counterexample exists below 100."
                    })
                })
        );
    }

    #[tokio::test]
    async fn agent_message_is_bound_to_the_active_sender_turn() {
        let (state, parent, parent_branch, _file) = fixture();
        let (child, _child_branch) = state
            .runtime
            .spawn_child_session(
                parent.session_id,
                parent_branch.branch_id,
                None,
                "inspect the proof".to_owned(),
                None,
                None,
                None,
            )
            .expect("child session");
        let admitted = state
            .runtime
            .admit_user_message(
                &parent,
                &parent_branch,
                bt_core::Message::text(Role::User, "coordinate the proof"),
            )
            .expect("admit parent turn");
        let bt_runtime::UserMessageAdmission::Started(admitted) = admitted else {
            panic!("parent turn should start");
        };
        let tools = registry(state.clone(), parent.session_id, parent_branch.branch_id);
        let result = tools
            .get("send_agent_message")
            .expect("send tool")
            .execute(
                json!({
                    "call_id": "call-causal",
                    "target_session_id": child.session_id,
                    "text": "check the induction step"
                }),
                context(),
            )
            .await
            .expect("send causal message");
        let message_id =
            parse_related_message_id(result.output["message_id"].as_str().expect("message id"))
                .expect("parse message id");
        let sent_event = state
            .runtime
            .all_events(parent.session_id)
            .expect("parent events")
            .into_iter()
            .find(|event| {
                matches!(
                    &event.payload,
                    bt_core::EventPayload::RelatedSessionMessageRecorded { message, .. }
                        if message.message_id == message_id
                )
            })
            .expect("sent event");
        assert_eq!(sent_event.turn_id, Some(admitted.turn_id()));
    }

    #[tokio::test]
    async fn agent_message_requires_an_active_sender_turn() {
        let (state, parent, parent_branch, _file) = fixture();
        let (child, _child_branch) = state
            .runtime
            .spawn_child_session(
                parent.session_id,
                parent_branch.branch_id,
                None,
                "inspect the proof".to_owned(),
                None,
                None,
                None,
            )
            .expect("child session");
        let tools = registry(state, parent.session_id, parent_branch.branch_id);
        let error = tools
            .get("send_agent_message")
            .expect("send tool")
            .execute(
                json!({
                    "call_id": "call-without-turn",
                    "target_session_id": child.session_id,
                    "text": "this must retain caller provenance"
                }),
                context(),
            )
            .await
            .expect_err("agent message without active caller turn must fail");
        assert!(error.to_string().contains("active caller turn"));
    }

    #[tokio::test]
    async fn startup_recovery_claims_a_pending_child_wake() {
        let (state, parent, parent_branch, _file) = fixture();
        let (child, _child_branch, receipt) = state
            .runtime
            .spawn_child_session_with_initial_objective(
                parent.session_id,
                parent_branch.branch_id,
                None,
                "resume the delegated proof".to_owned(),
                Some("recovery-check".to_owned()),
                None,
                None,
            )
            .expect("spawn child with pending objective");
        assert_eq!(
            state
                .runtime
                .all_pending_related_session_messages()
                .expect("pending messages")
                .len(),
            1
        );

        assert_eq!(
            recover_pending_agent_turns(state.clone()).expect("recover pending child"),
            1
        );
        let record = state
            .runtime
            .related_session_messages(child.session_id)
            .expect("child mailbox")
            .into_iter()
            .find(|record| record.message.message_id == receipt.message_id)
            .expect("initial objective record");
        assert_eq!(record.status, RelatedSessionMessageStatus::Claimed);
        assert!(record.resulting_turn_id.is_some());
    }

    #[tokio::test]
    async fn agent_message_tool_rejects_unrelated_sessions() {
        let (state, session, branch, _file) = fixture();
        let (unrelated, _unrelated_branch) = state
            .runtime
            .create_session(
                "/tmp/project".into(),
                ConnectionId::new("local"),
                None,
                SessionToolMode::Extended,
                Some("unrelated".to_owned()),
                None,
            )
            .expect("unrelated session");
        let tools = registry(state, session.session_id, branch.branch_id);
        let error = tools
            .get("send_agent_message")
            .expect("send tool")
            .execute(
                json!({
                    "call_id": "call-send",
                    "target_session_id": unrelated.session_id,
                    "text": "cross the lineage boundary"
                }),
                context(),
            )
            .await
            .expect_err("unrelated session must be rejected");
        assert!(error.to_string().contains("same lineage tree"));
    }
}
