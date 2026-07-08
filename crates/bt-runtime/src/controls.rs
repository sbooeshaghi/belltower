use bt_core::{BranchId, Message, Result, SessionId};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use time::OffsetDateTime;

#[derive(Default)]
pub struct ControlQueues {
    inner: Mutex<HashMap<SessionId, SessionControlState>>,
}

#[derive(Clone, Debug, Default)]
pub struct SessionControlState {
    pub cancelled: bool,
    pub steer_messages: VecDeque<String>,
    pub queued_messages: VecDeque<QueuedSessionMessage>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QueuedSessionMessage {
    pub branch_id: BranchId,
    pub message: Message,
    pub enqueued_at: OffsetDateTime,
}

impl ControlQueues {
    pub fn cancel(&self, session_id: SessionId) -> Result<()> {
        let mut guard = self.inner.lock().map_err(|_| {
            bt_core::BelltowerError::InvalidState("control lock poisoned".to_owned())
        })?;
        let state = guard.entry(session_id).or_default();
        state.cancelled = true;
        Ok(())
    }

    pub fn steer(&self, session_id: SessionId, message: String) -> Result<()> {
        let mut guard = self.inner.lock().map_err(|_| {
            bt_core::BelltowerError::InvalidState("control lock poisoned".to_owned())
        })?;
        let state = guard.entry(session_id).or_default();
        state.steer_messages.push_back(message);
        Ok(())
    }

    pub fn drain_steer_messages(&self, session_id: SessionId) -> Result<Vec<String>> {
        let mut guard = self.inner.lock().map_err(|_| {
            bt_core::BelltowerError::InvalidState("control lock poisoned".to_owned())
        })?;
        let state = guard.entry(session_id).or_default();
        Ok(state.steer_messages.drain(..).collect())
    }

    pub fn is_cancelled(&self, session_id: SessionId) -> Result<bool> {
        let guard = self.inner.lock().map_err(|_| {
            bt_core::BelltowerError::InvalidState("control lock poisoned".to_owned())
        })?;
        Ok(guard.get(&session_id).is_some_and(|state| state.cancelled))
    }

    pub fn take_cancelled(&self, session_id: SessionId) -> Result<bool> {
        let mut guard = self.inner.lock().map_err(|_| {
            bt_core::BelltowerError::InvalidState("control lock poisoned".to_owned())
        })?;
        let state = guard.entry(session_id).or_default();
        let cancelled = state.cancelled;
        state.cancelled = false;
        Ok(cancelled)
    }

    pub fn snapshot(&self, session_id: SessionId) -> Result<SessionControlState> {
        let guard = self.inner.lock().map_err(|_| {
            bt_core::BelltowerError::InvalidState("control lock poisoned".to_owned())
        })?;
        Ok(guard.get(&session_id).cloned().unwrap_or_default())
    }

    pub fn enqueue_message(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        message: Message,
    ) -> Result<usize> {
        let mut guard = self.inner.lock().map_err(|_| {
            bt_core::BelltowerError::InvalidState("control lock poisoned".to_owned())
        })?;
        let state = guard.entry(session_id).or_default();
        state.queued_messages.push_back(QueuedSessionMessage {
            branch_id,
            message,
            enqueued_at: OffsetDateTime::now_utc(),
        });
        Ok(state.queued_messages.len())
    }

    pub fn pop_next_message(&self, session_id: SessionId) -> Result<Option<QueuedSessionMessage>> {
        let mut guard = self.inner.lock().map_err(|_| {
            bt_core::BelltowerError::InvalidState("control lock poisoned".to_owned())
        })?;
        let state = guard.entry(session_id).or_default();
        Ok(state.queued_messages.pop_front())
    }

    pub fn clear_queued_messages(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<QueuedSessionMessage>> {
        let mut guard = self.inner.lock().map_err(|_| {
            bt_core::BelltowerError::InvalidState("control lock poisoned".to_owned())
        })?;
        let state = guard.entry(session_id).or_default();
        Ok(state.queued_messages.drain(..).collect())
    }
}
