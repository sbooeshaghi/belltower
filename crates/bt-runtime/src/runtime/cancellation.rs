//! Runtime-owned exact-turn cancellation signaling.
//!
//! The event log remains authoritative. This module only aligns an in-memory
//! wake signal with the active `(session, branch, turn)` projection so live
//! work converges promptly on the committed cancellation state.

use super::support::apply_turn_id;
use super::*;

impl BelltowerRuntime {
    pub fn cancel_session(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        reason: impl Into<String>,
    ) -> Result<i64> {
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::SessionCancelled {
                reason: reason.into(),
            },
        );
        self.ensure_session_started_for(session_id, branch_id)?;
        self.take_store_append_fault_for_test()?;
        let mut store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        let active_claim = store.active_turn_claim(session_id)?;
        if let Some((active_branch_id, _)) = active_claim
            && active_branch_id != branch_id
        {
            return Err(bt_core::BelltowerError::Protocol(format!(
                "branch {branch_id} does not own the active turn on branch {active_branch_id}"
            )));
        }
        let seq_id = store.append_event(&event)?;

        // The canonical append happens first. While the store serialization
        // boundary is still held, wake only the exact in-memory owner that
        // was active at that append. A newly admitted turn can never inherit
        // an older signal.
        if let Some((active_branch_id, active_turn_id)) = active_claim
            && let Some(entry) = self
                .active_cancellations
                .lock()
                .map_err(|_| {
                    bt_core::BelltowerError::InvalidState(
                        "active cancellation lock poisoned".to_owned(),
                    )
                })?
                .get(&session_id)
            && entry.branch_id == active_branch_id
            && entry.turn_id == active_turn_id
        {
            entry.signal.cancel();
        }
        self.publish_committed_event(event, seq_id);
        drop(store);
        Ok(seq_id)
    }

    /// Registers the process-local cancellation capability for one exact
    /// active claim. Registration and canonical cancel inspection share the
    /// store serialization boundary with `cancel_session`, closing the race
    /// where a durable request could otherwise arrive just before registration.
    pub fn begin_active_turn_cancellation(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        turn_id: TurnId,
    ) -> Result<ActiveTurnCancellation<'_>> {
        let store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        if store.active_turn_claim(session_id)? != Some((branch_id, turn_id)) {
            return Err(bt_core::BelltowerError::InvalidState(format!(
                "turn {turn_id} on branch {branch_id} is not the active owner"
            )));
        }
        let signal = bt_core::CancellationSignal::new();
        if store
            .load_session_control(session_id)?
            .is_some_and(|control| control.cancel_requested)
        {
            signal.cancel();
        }
        self.active_cancellations
            .lock()
            .map_err(|_| {
                bt_core::BelltowerError::InvalidState(
                    "active cancellation lock poisoned".to_owned(),
                )
            })?
            .insert(
                session_id,
                ActiveCancellationEntry {
                    branch_id,
                    turn_id,
                    signal: signal.clone(),
                },
            );
        drop(store);
        Ok(ActiveTurnCancellation {
            runtime: self,
            session_id,
            branch_id,
            turn_id,
            signal,
        })
    }

    pub fn active_turn_cancellation_signal(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        turn_id: TurnId,
    ) -> Result<Option<bt_core::CancellationSignal>> {
        Ok(self
            .active_cancellations
            .lock()
            .map_err(|_| {
                bt_core::BelltowerError::InvalidState(
                    "active cancellation lock poisoned".to_owned(),
                )
            })?
            .get(&session_id)
            .filter(|entry| entry.branch_id == branch_id && entry.turn_id == turn_id)
            .map(|entry| entry.signal.clone()))
    }

    /// Atomically admits a model-requested tool call only while canonical
    /// session control has no pending cancellation.
    pub fn record_tool_request_transition(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        turn_id: TurnId,
        call_id: ToolCallId,
        tool_name: String,
        arguments: serde_json::Value,
        operation: bt_core::ToolOperationContext,
        message: Message,
    ) -> Result<bool> {
        let events = tool_request_events(
            session_id, branch_id, turn_id, call_id, tool_name, arguments, operation, message,
        );
        self.ensure_session_started_for(session_id, branch_id)?;
        self.take_store_append_fault_for_test()?;
        let mut store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        if store
            .load_session_control(session_id)?
            .is_some_and(|control| control.cancel_requested)
        {
            return Ok(false);
        }
        let seq_ids = store.commit_active_turn_events(session_id, branch_id, turn_id, &events)?;
        for (event, seq_id) in events.into_iter().zip(seq_ids.iter().copied()) {
            self.publish_committed_event(event, seq_id);
        }
        Ok(true)
    }

    pub fn record_tool_operation_request_transition(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        call_id: ToolCallId,
        tool_name: String,
        arguments: serde_json::Value,
        operation: bt_core::ToolOperationContext,
        turn_id: Option<TurnId>,
    ) -> Result<()> {
        self.append_events(tool_operation_request_events(
            session_id, branch_id, call_id, tool_name, arguments, operation, turn_id,
        ))?;
        Ok(())
    }

    pub fn is_cancelled(&self, session_id: SessionId) -> Result<bool> {
        Ok(self
            .load_session_control(session_id)?
            .is_some_and(|state| state.cancel_requested))
    }
}

fn tool_request_events(
    session_id: SessionId,
    branch_id: bt_core::BranchId,
    turn_id: TurnId,
    call_id: ToolCallId,
    tool_name: String,
    arguments: serde_json::Value,
    operation: bt_core::ToolOperationContext,
    message: Message,
) -> Vec<EventEnvelope> {
    let mut events = tool_operation_request_events(
        session_id,
        branch_id,
        call_id,
        tool_name,
        arguments,
        operation,
        Some(turn_id),
    );
    events.push(apply_turn_id(
        EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended { message },
        ),
        Some(turn_id),
    ));
    events
}

#[allow(clippy::too_many_arguments)]
fn tool_operation_request_events(
    session_id: SessionId,
    branch_id: bt_core::BranchId,
    call_id: ToolCallId,
    tool_name: String,
    arguments: serde_json::Value,
    operation: bt_core::ToolOperationContext,
    turn_id: Option<TurnId>,
) -> Vec<EventEnvelope> {
    vec![
        apply_turn_id(
            EventEnvelope::new(
                session_id,
                branch_id,
                SpanKind::Tool,
                EventPayload::ToolOperationRecorded {
                    call_id: call_id.clone(),
                    tool_name: tool_name.clone(),
                    operation,
                },
            ),
            turn_id,
        ),
        apply_turn_id(
            EventEnvelope::new(
                session_id,
                branch_id,
                SpanKind::Tool,
                EventPayload::ToolCallRequested {
                    call_id,
                    tool_name,
                    arguments,
                },
            ),
            turn_id,
        ),
    ]
}
