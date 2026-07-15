use bt_core::{
    BelltowerError, BranchId, EventEnvelope, EventPayload, Result, Role, SessionId, TurnId,
    TurnStartSource,
};

fn invalid_transition(message: impl Into<String>) -> BelltowerError {
    BelltowerError::InvalidState(message.into())
}

fn validate_event_scope(
    session_id: SessionId,
    branch_id: BranchId,
    events: &[EventEnvelope],
) -> Result<()> {
    if events
        .iter()
        .any(|event| event.session_id != session_id || event.branch_id != branch_id)
    {
        return Err(invalid_transition(
            "control-plane transition events must share one session and branch",
        ));
    }
    Ok(())
}

fn validate_message_and_turn_start(
    events: &[EventEnvelope],
    expected_revision_id: Option<u64>,
    expected_source: TurnStartSource,
) -> Result<(BranchId, TurnId)> {
    let mut message_turn_id = None;
    let mut started = None;
    for event in events {
        match &event.payload {
            EventPayload::MessageAppended { message } => {
                if message.role != Role::User {
                    return Err(invalid_transition(
                        "turn transition may contain only its user input message",
                    ));
                }
                if message_turn_id.replace(event.turn_id).is_some() || event.turn_id.is_none() {
                    return Err(invalid_transition(
                        "turn transition must contain exactly one turn-bound user message",
                    ));
                }
            }
            EventPayload::TurnStarted {
                turn_id,
                settings_revision_id,
                source,
                ..
            } => {
                if started
                    .replace((event.branch_id, *turn_id, *settings_revision_id, source))
                    .is_some()
                    || event.turn_id != Some(*turn_id)
                {
                    return Err(invalid_transition(
                        "turn transition must contain exactly one self-consistent turn.started event",
                    ));
                }
            }
            _ => {}
        }
    }
    let Some(message_turn_id) = message_turn_id.flatten() else {
        return Err(invalid_transition(
            "turn transition is missing its turn-bound user message",
        ));
    };
    let Some((branch_id, turn_id, settings_revision_id, source)) = started else {
        return Err(invalid_transition(
            "turn transition is missing its turn.started event",
        ));
    };
    if message_turn_id != turn_id {
        return Err(invalid_transition(
            "turn transition message and turn.started event use different turn ids",
        ));
    }
    if source != &expected_source {
        return Err(invalid_transition(
            "turn transition uses an unexpected start source",
        ));
    }
    if expected_revision_id.is_some_and(|expected| expected != settings_revision_id) {
        return Err(invalid_transition(
            "turn transition uses an unexpected settings revision",
        ));
    }
    Ok((branch_id, turn_id))
}

pub(super) fn validate_turn_admission_events(
    session_id: SessionId,
    expected_settings_revision_id: u64,
    cancel_clear_event: &EventEnvelope,
    started_events: &[EventEnvelope],
    queued_events: &[EventEnvelope],
) -> Result<()> {
    let (branch_id, _) = validate_message_and_turn_start(
        started_events,
        Some(expected_settings_revision_id),
        TurnStartSource::UserMessage,
    )?;
    validate_event_scope(session_id, branch_id, started_events)?;
    validate_event_scope(session_id, branch_id, queued_events)?;
    validate_event_scope(
        session_id,
        branch_id,
        std::slice::from_ref(cancel_clear_event),
    )?;
    if !matches!(
        cancel_clear_event.payload,
        EventPayload::SessionCancelCleared { .. }
    ) {
        return Err(invalid_transition(
            "turn admission cancel event must be session.cancel.cleared",
        ));
    }
    let mut queued_count = 0;
    for event in queued_events {
        match &event.payload {
            EventPayload::SessionQueuedMessageEnqueued {
                settings_revision_id,
                ..
            } => {
                queued_count += 1;
                if *settings_revision_id != expected_settings_revision_id {
                    return Err(invalid_transition(
                        "queued admission uses an unexpected settings revision",
                    ));
                }
            }
            EventPayload::OperatorCommandRecorded { .. } => {}
            _ => {
                return Err(invalid_transition(
                    "queued admission contains an unexpected event",
                ));
            }
        }
    }
    if queued_count != 1 {
        return Err(invalid_transition(
            "queued admission must contain one queued message and optional operator audit events",
        ));
    }
    if started_events.iter().any(|event| {
        !matches!(
            event.payload,
            EventPayload::MessageAppended { .. } | EventPayload::TurnStarted { .. }
        )
    }) {
        return Err(invalid_transition(
            "started admission may contain only its user message and turn.started event",
        ));
    }
    Ok(())
}

pub(super) fn validate_queued_continuation_events(
    session_id: SessionId,
    queue_event_id: bt_core::EventId,
    expected_settings_revision_id: u64,
    events: &[EventEnvelope],
) -> Result<()> {
    let (branch_id, _) = validate_message_and_turn_start(
        events,
        Some(expected_settings_revision_id),
        TurnStartSource::QueuedFollowUp,
    )?;
    validate_event_scope(session_id, branch_id, events)?;
    let mut resolution_count = 0;
    for event in events {
        match &event.payload {
            EventPayload::SessionQueuedMessageResolved {
                queue_event_id: resolved_id,
                outcome: bt_core::QueuedMessageResolutionOutcome::Dispatched,
                ..
            } if *resolved_id == queue_event_id => resolution_count += 1,
            EventPayload::SessionQueuedMessageResolved { .. } => {
                return Err(invalid_transition(
                    "queued continuation resolves an unexpected source or outcome",
                ));
            }
            EventPayload::OperatorCommandRecorded { .. }
            | EventPayload::MessageAppended { .. }
            | EventPayload::TurnStarted { .. } => {}
            _ => {
                return Err(invalid_transition(
                    "queued continuation contains an unexpected event",
                ));
            }
        }
    }
    if resolution_count != 1 {
        return Err(invalid_transition(
            "queued continuation must resolve its source and start exactly one turn",
        ));
    }
    Ok(())
}

pub(super) fn validate_steer_continuation_events(
    session_id: SessionId,
    branch_id: BranchId,
    steer_event_ids: &[bt_core::EventId],
    expected_settings_revision_id: u64,
    events: &[EventEnvelope],
) -> Result<()> {
    let (event_branch_id, _) = validate_message_and_turn_start(
        events,
        Some(expected_settings_revision_id),
        TurnStartSource::SteerFollowUp,
    )?;
    if event_branch_id != branch_id {
        return Err(invalid_transition(
            "steer continuation events target the wrong branch",
        ));
    }
    validate_event_scope(session_id, branch_id, events)?;
    let mut resolution_count = 0;
    for event in events {
        match &event.payload {
            EventPayload::SessionSteersResolved {
                steer_event_ids: resolved_ids,
                outcome: bt_core::SteerResolutionOutcome::Applied,
                ..
            } if resolved_ids == steer_event_ids => resolution_count += 1,
            EventPayload::SessionSteersResolved { .. } => {
                return Err(invalid_transition(
                    "steer continuation resolves unexpected sources or outcome",
                ));
            }
            EventPayload::MessageAppended { .. } | EventPayload::TurnStarted { .. } => {}
            _ => {
                return Err(invalid_transition(
                    "steer continuation contains an unexpected event",
                ));
            }
        }
    }
    if resolution_count != 1 {
        return Err(invalid_transition(
            "steer continuation must resolve its exact sources and start one turn",
        ));
    }
    Ok(())
}
