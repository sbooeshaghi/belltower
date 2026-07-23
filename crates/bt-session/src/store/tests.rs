//! Tests for SQLite session store persistence, projections, replay, and export fidelity.

use super::{ContinuationClaim, ResumedContinuationKind, SessionTurnAdmission, SqliteSessionStore};
use crate::{
    LEGACY_SESSION_EXPORT_BUNDLE_SCHEMA_VERSION, LegacySessionExporter,
    PortableSessionBundleExporter, PortableSessionBundleImporter, SESSION_BT_ARTIFACTS_DIR,
    SESSION_BT_CHECKSUMS, SESSION_BT_EVENTS, SESSION_BT_MANIFEST, SESSION_BT_PARENT_NODE_REF_ATTR,
    SESSION_BT_RAW_CHUNK_REFS, SessionBundleArtifactInput, SessionBundleDiffRelationship,
    SessionBundleEventRecord, SessionBundleExportOptions, continue_session_bundle_directory,
    diff_session_bundle_directories, instruction_provenance_for_turn,
    validate_session_bundle_directory,
};
use bt_core::{
    ApprovalDecision, ApprovalDecisionSource, ApprovalScope, BelltowerError, BranchRecord,
    BudgetConfig, CompletionRequest, ConnectionId, ContextManifest, ContextMessageSourceRef,
    EventEnvelope, EventId, EventPayload, InstructionDocument, MAX_RELATED_SESSION_DESCENDANTS,
    Message, PortableHash, RelatedSessionDeliveryMode, RelatedSessionMessage,
    RelatedSessionMessageDirection, RelatedSessionMessageId, RelatedSessionMessageKind,
    RelatedSessionMessageStatus, Role, SessionBundleArtifactMode, SessionBundleManifest,
    SessionNodeRef, SessionRecord, SessionStatus, SessionToolMode, SpanKind, ToolCallId,
    ToolOperationContext, ToolResultEnvelope, TurnId, TurnInstructionProvenance, TurnStartSource,
    default_settings_revision_id,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use tempfile::{NamedTempFile, tempdir};
use time::OffsetDateTime;

fn sample_session() -> (SessionRecord, BranchRecord) {
    let session = SessionRecord {
        session_id: bt_core::SessionId::new(),
        project_root: "/tmp/project".into(),
        connection_id: ConnectionId::new("local"),
        model_id: None,
        settings_revision_id: default_settings_revision_id(),
        created_at: OffsetDateTime::now_utc(),
        updated_at: OffsetDateTime::now_utc(),
        status: SessionStatus::Active,
        tool_mode: SessionToolMode::Extended,
        display_name: Some("demo".to_owned()),
        objective: None,
        parent_session_id: None,
        parent_branch_id: None,
        parent_turn_id: None,
    };
    let branch = BranchRecord {
        branch_id: bt_core::BranchId::new(),
        session_id: session.session_id,
        parent_branch_id: None,
        parent_event_id: None,
        head_event_id: None,
        summary: None,
        created_at: OffsetDateTime::now_utc(),
        is_default: true,
    };
    (session, branch)
}

fn turn_started_event(
    session: &SessionRecord,
    branch: &BranchRecord,
    turn_id: TurnId,
) -> EventEnvelope {
    turn_started_event_with_source(session, branch, turn_id, TurnStartSource::UserMessage)
}

fn turn_started_event_with_source(
    session: &SessionRecord,
    branch: &BranchRecord,
    turn_id: TurnId,
    source: TurnStartSource,
) -> EventEnvelope {
    turn_started_event_with_source_and_revision(
        session,
        branch,
        turn_id,
        source,
        session.settings_revision_id,
    )
}

fn turn_started_event_with_source_and_revision(
    session: &SessionRecord,
    branch: &BranchRecord,
    turn_id: TurnId,
    source: TurnStartSource,
    settings_revision_id: u64,
) -> EventEnvelope {
    EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::TurnStarted {
            turn_id,
            provider: "test".to_owned(),
            model: "test-model".to_owned(),
            message_count: 1,
            settings_revision_id,
            source,
            resumed_from_call_id: None,
        },
    )
    .with_turn_id(turn_id)
}

fn turn_finished_event(
    session: &SessionRecord,
    branch: &BranchRecord,
    turn_id: TurnId,
) -> EventEnvelope {
    EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::TurnFinished {
            turn_id,
            provider: "test".to_owned(),
            model: "test-model".to_owned(),
            status: "completed".to_owned(),
            finish_reason: Some("stop".to_owned()),
            latency_ms: 1,
        },
    )
    .with_turn_id(turn_id)
}

fn queued_message_event(
    session: &SessionRecord,
    branch: &BranchRecord,
    text: &str,
) -> EventEnvelope {
    EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::SessionQueuedMessageEnqueued {
            message: Message::text(Role::User, text),
            settings_revision_id: session.settings_revision_id,
        },
    )
}

fn cancel_clear_event(session: &SessionRecord, branch: &BranchRecord) -> EventEnvelope {
    EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::SessionCancelCleared {
            reason: "superseded by direct user operation".to_owned(),
        },
    )
}

fn child_session(
    parent: &SessionRecord,
    parent_branch: &BranchRecord,
) -> (SessionRecord, BranchRecord) {
    let (mut child, child_branch) = sample_session();
    child.parent_session_id = Some(parent.session_id);
    child.parent_branch_id = Some(parent_branch.branch_id);
    child.objective = Some("investigate the delegated question".to_owned());
    (child, child_branch)
}

fn related_message_events(
    source: &SessionRecord,
    source_branch: &BranchRecord,
    destination: &SessionRecord,
    destination_branch: &BranchRecord,
    message_id: RelatedSessionMessageId,
    text: &str,
    delivery_mode: RelatedSessionDeliveryMode,
) -> (EventEnvelope, EventEnvelope) {
    let message = RelatedSessionMessage {
        message_id,
        context_message_id: bt_core::MessageId::new(),
        source_session_id: source.session_id,
        source_branch_id: source_branch.branch_id,
        caused_by_turn_id: None,
        destination_session_id: destination.session_id,
        destination_branch_id: destination_branch.branch_id,
        kind: RelatedSessionMessageKind::Instruction,
        delivery_mode,
        in_reply_to: None,
        text: text.to_owned(),
        artifact_refs: Vec::new(),
        created_at: OffsetDateTime::now_utc(),
    };
    let sent_event_id = EventId::new();
    let received_event_id = EventId::new();
    let mut sent = EventEnvelope::new(
        source.session_id,
        source_branch.branch_id,
        SpanKind::Chain,
        EventPayload::RelatedSessionMessageRecorded {
            direction: RelatedSessionMessageDirection::Sent,
            counterpart_event_id: received_event_id,
            message: message.clone(),
        },
    );
    sent.event_id = sent_event_id;
    let mut received = EventEnvelope::new(
        destination.session_id,
        destination_branch.branch_id,
        SpanKind::Chain,
        EventPayload::RelatedSessionMessageRecorded {
            direction: RelatedSessionMessageDirection::Received,
            counterpart_event_id: sent_event_id,
            message,
        },
    );
    received.event_id = received_event_id;
    (sent, received)
}

fn child_spawn_events(
    parent: &SessionRecord,
    parent_branch: &BranchRecord,
    child: &SessionRecord,
    child_branch: &BranchRecord,
) -> [EventEnvelope; 4] {
    let objective = child.objective.clone().expect("child objective");
    [
        EventEnvelope::new(
            parent.session_id,
            parent_branch.branch_id,
            SpanKind::Chain,
            EventPayload::SessionSpawnRequested {
                child_session_id: child.session_id,
                objective: objective.clone(),
                connection_id: child.connection_id.to_string(),
                model_id: child.model_id.clone(),
            },
        ),
        EventEnvelope::new(
            child.session_id,
            child_branch.branch_id,
            SpanKind::Session,
            EventPayload::SessionStarted {
                project_root: child.project_root.to_string(),
                connection_id: child.connection_id.to_string(),
            },
        ),
        EventEnvelope::new(
            child.session_id,
            child_branch.branch_id,
            SpanKind::Chain,
            EventPayload::SessionHandoffRecorded {
                parent_session_id: parent.session_id,
                parent_branch_id: parent_branch.branch_id,
                parent_turn_id: child.parent_turn_id,
                objective: objective.clone(),
                summary: objective.clone(),
            },
        ),
        EventEnvelope::new(
            parent.session_id,
            parent_branch.branch_id,
            SpanKind::Chain,
            EventPayload::SessionSpawned {
                child_session_id: child.session_id,
                child_branch_id: child_branch.branch_id,
                objective,
            },
        ),
    ]
}

#[test]
fn migrations_and_session_creation_work() {
    let file = NamedTempFile::new().expect("tempfile");
    let mut store = SqliteSessionStore::open(file.path()).expect("open store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let loaded = store
        .load_session(session.session_id)
        .expect("load session")
        .expect("session present");
    assert_eq!(loaded.session_id, session.session_id);
    assert!(
        !store
            .session_has_events(session.session_id)
            .expect("event presence")
    );
}

#[test]
fn related_session_delivery_is_atomic_idempotent_and_lineage_scoped() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (parent, parent_branch) = sample_session();
    store
        .create_session(&parent, &parent_branch)
        .expect("create parent");
    let (child, child_branch) = child_session(&parent, &parent_branch);
    store
        .create_session(&child, &child_branch)
        .expect("create child");

    let message_id = RelatedSessionMessageId::new();
    let (sent, received) = related_message_events(
        &parent,
        &parent_branch,
        &child,
        &child_branch,
        message_id,
        "test the counterexample",
        RelatedSessionDeliveryMode::Wake,
    );
    let receipt = store
        .commit_related_session_message(&sent, &received)
        .expect("commit message");
    let retried = store
        .commit_related_session_message(&sent, &received)
        .expect("retry message");
    assert_eq!(receipt, retried);
    assert_eq!(receipt.status, RelatedSessionMessageStatus::Pending);

    let parent_messages = store
        .load_related_session_messages(parent.session_id)
        .expect("parent messages");
    let child_messages = store
        .load_related_session_messages(child.session_id)
        .expect("child messages");
    assert_eq!(parent_messages.len(), 1);
    assert_eq!(child_messages.len(), 1);
    assert_eq!(
        parent_messages[0].direction,
        RelatedSessionMessageDirection::Sent
    );
    assert_eq!(
        child_messages[0].direction,
        RelatedSessionMessageDirection::Received
    );

    let (changed_sent, changed_received) = related_message_events(
        &parent,
        &parent_branch,
        &child,
        &child_branch,
        message_id,
        "different content",
        RelatedSessionDeliveryMode::Wake,
    );
    assert!(matches!(
        store.commit_related_session_message(&changed_sent, &changed_received),
        Err(BelltowerError::Protocol(_))
    ));

    let (unrelated, unrelated_branch) = sample_session();
    store
        .create_session(&unrelated, &unrelated_branch)
        .expect("create unrelated session");
    let (unrelated_sent, unrelated_received) = related_message_events(
        &parent,
        &parent_branch,
        &unrelated,
        &unrelated_branch,
        RelatedSessionMessageId::new(),
        "this must not cross unrelated sessions",
        RelatedSessionDeliveryMode::Notify,
    );
    assert!(matches!(
        store.commit_related_session_message(&unrelated_sent, &unrelated_received),
        Err(BelltowerError::Protocol(_))
    ));
}

#[test]
fn related_session_wake_survives_restart_and_claims_in_order() {
    let file = NamedTempFile::new().expect("tempfile");
    let (parent, parent_branch) = sample_session();
    let (child, child_branch) = child_session(&parent, &parent_branch);
    let first_message_id = RelatedSessionMessageId::new();
    let second_message_id = RelatedSessionMessageId::new();
    {
        let mut store = SqliteSessionStore::open(file.path()).expect("store");
        store
            .create_session(&parent, &parent_branch)
            .expect("create parent");
        store
            .create_session(&child, &child_branch)
            .expect("create child");
        for (message_id, text) in [
            (first_message_id, "first instruction"),
            (second_message_id, "second instruction"),
        ] {
            let (sent, received) = related_message_events(
                &parent,
                &parent_branch,
                &child,
                &child_branch,
                message_id,
                text,
                RelatedSessionDeliveryMode::Wake,
            );
            store
                .commit_related_session_message(&sent, &received)
                .expect("commit wake message");
        }
    }

    let mut store = SqliteSessionStore::open(file.path()).expect("reopen store");
    let pending = store
        .load_pending_related_session_messages(child.session_id)
        .expect("pending messages");
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].message.message_id, first_message_id);

    let stale_turn_id = TurnId::new();
    let stale_events = vec![
        EventEnvelope::new(
            child.session_id,
            child_branch.branch_id,
            SpanKind::Chain,
            EventPayload::RelatedSessionMessageResolved {
                message_id: second_message_id,
                status: RelatedSessionMessageStatus::Claimed,
                resulting_turn_id: Some(stale_turn_id),
                reason: None,
            },
        ),
        turn_started_event_with_source(
            &child,
            &child_branch,
            stale_turn_id,
            TurnStartSource::RelatedSessionMessage,
        ),
    ];
    assert_eq!(
        store
            .claim_related_message_continuation(
                child.session_id,
                second_message_id,
                child.settings_revision_id,
                &stale_events,
            )
            .expect("out-of-order claim"),
        ContinuationClaim::Stale
    );

    let turn_id = TurnId::new();
    let claim_events = vec![
        EventEnvelope::new(
            child.session_id,
            child_branch.branch_id,
            SpanKind::Chain,
            EventPayload::RelatedSessionMessageResolved {
                message_id: first_message_id,
                status: RelatedSessionMessageStatus::Claimed,
                resulting_turn_id: Some(turn_id),
                reason: None,
            },
        ),
        turn_started_event_with_source(
            &child,
            &child_branch,
            turn_id,
            TurnStartSource::RelatedSessionMessage,
        ),
    ];
    assert!(matches!(
        store
            .claim_related_message_continuation(
                child.session_id,
                first_message_id,
                child.settings_revision_id,
                &claim_events,
            )
            .expect("claim first message"),
        ContinuationClaim::Claimed { .. }
    ));
    let pending = store
        .load_pending_related_session_messages(child.session_id)
        .expect("remaining pending messages");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].message.message_id, second_message_id);
    let sender_messages = store
        .load_related_session_messages(parent.session_id)
        .expect("sender messages");
    assert_eq!(
        sender_messages[0].status,
        RelatedSessionMessageStatus::Claimed
    );
    assert_eq!(sender_messages[0].resulting_turn_id, Some(turn_id));
}

#[test]
fn failed_child_spawn_with_initial_message_rolls_back_everything() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (parent_session, parent_branch) = sample_session();
    store
        .create_session(&parent_session, &parent_branch)
        .expect("create parent session");
    let (mut child_session, child_branch) = sample_session();
    child_session.parent_session_id = Some(parent_session.session_id);
    child_session.parent_branch_id = Some(parent_branch.branch_id);
    child_session.objective = Some("inspect the evidence".to_owned());

    let spawn_requested = EventEnvelope::new(
        parent_session.session_id,
        parent_branch.branch_id,
        SpanKind::Chain,
        EventPayload::SessionSpawnRequested {
            child_session_id: child_session.session_id,
            objective: "inspect the evidence".to_owned(),
            connection_id: child_session.connection_id.to_string(),
            model_id: child_session.model_id.clone(),
        },
    );
    let child_started = EventEnvelope::new(
        child_session.session_id,
        child_branch.branch_id,
        SpanKind::Session,
        EventPayload::SessionStarted {
            project_root: child_session.project_root.to_string(),
            connection_id: child_session.connection_id.to_string(),
        },
    );
    let child_handoff = EventEnvelope::new(
        child_session.session_id,
        child_branch.branch_id,
        SpanKind::Chain,
        EventPayload::SessionHandoffRecorded {
            parent_session_id: parent_session.session_id,
            parent_branch_id: parent_branch.branch_id,
            parent_turn_id: None,
            objective: "inspect the evidence".to_owned(),
            summary: "inspect the evidence".to_owned(),
        },
    );
    let mut spawn_completed = EventEnvelope::new(
        parent_session.session_id,
        parent_branch.branch_id,
        SpanKind::Chain,
        EventPayload::SessionSpawned {
            child_session_id: child_session.session_id,
            child_branch_id: child_branch.branch_id,
            objective: "inspect the evidence".to_owned(),
        },
    );
    let (sent, received) = related_message_events(
        &parent_session,
        &parent_branch,
        &child_session,
        &child_branch,
        RelatedSessionMessageId::new(),
        "inspect the evidence",
        RelatedSessionDeliveryMode::Wake,
    );
    spawn_completed.event_id = sent.event_id;

    let error = store
        .commit_child_session_spawn_with_initial_message(
            &child_session,
            &child_branch,
            &spawn_requested,
            &child_started,
            &child_handoff,
            &spawn_completed,
            &sent,
            &received,
        )
        .expect_err("duplicate event id must fail the spawn transaction");
    assert!(matches!(error, BelltowerError::Storage(_)));
    assert!(
        store
            .load_session(child_session.session_id)
            .expect("child lookup")
            .is_none()
    );
    assert!(
        store
            .load_all_events(parent_session.session_id)
            .expect("parent events")
            .is_empty()
    );
    assert!(
        store
            .load_all_events(child_session.session_id)
            .expect("child events")
            .is_empty()
    );
}

#[test]
fn child_spawn_transaction_enforces_lineage_descendant_limit() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (parent, parent_branch) = sample_session();
    store
        .create_session(&parent, &parent_branch)
        .expect("create parent");

    for _ in 0..MAX_RELATED_SESSION_DESCENDANTS {
        let (child, child_branch) = child_session(&parent, &parent_branch);
        let events = child_spawn_events(&parent, &parent_branch, &child, &child_branch);
        store
            .commit_child_session_spawn(
                &child,
                &child_branch,
                &events[0],
                &events[1],
                &events[2],
                &events[3],
            )
            .expect("spawn within lineage bound");
    }

    let (overflow, overflow_branch) = child_session(&parent, &parent_branch);
    let events = child_spawn_events(&parent, &parent_branch, &overflow, &overflow_branch);
    let error = store
        .commit_child_session_spawn(
            &overflow,
            &overflow_branch,
            &events[0],
            &events[1],
            &events[2],
            &events[3],
        )
        .expect_err("the store must reject a lineage overflow");
    assert!(error.to_string().contains("subagent lineage limit"));
    assert!(
        store
            .load_session(overflow.session_id)
            .expect("overflow child lookup")
            .is_none()
    );
}

#[test]
fn session_has_events_reflects_appended_activity() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    assert!(
        !store
            .session_has_events(session.session_id)
            .expect("no events before append")
    );

    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::User, "hello"),
            },
        ))
        .expect("append message");

    assert!(
        store
            .session_has_events(session.session_id)
            .expect("events after append")
    );
}

#[test]
fn turn_admission_starts_once_then_queues_and_claims_oldest_continuation() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let first_turn_id = TurnId::new();
    let first_message = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::MessageAppended {
            message: Message::text(Role::User, "first"),
        },
    )
    .with_turn_id(first_turn_id);
    let first_start = turn_started_event(&session, &branch, first_turn_id);
    let first_queue = queued_message_event(&session, &branch, "first");
    let admission = store
        .admit_turn_or_queue(
            session.session_id,
            session.settings_revision_id,
            &cancel_clear_event(&session, &branch),
            &[first_message, first_start],
            &[first_queue],
        )
        .expect("admit first turn");
    assert!(matches!(
        admission,
        SessionTurnAdmission::Started { ref seq_ids } if seq_ids.len() == 2
    ));

    let second_turn_id = TurnId::new();
    let second_message = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::MessageAppended {
            message: Message::text(Role::User, "second"),
        },
    )
    .with_turn_id(second_turn_id);
    let second_start = turn_started_event(&session, &branch, second_turn_id);
    let second_queue = queued_message_event(&session, &branch, "second");
    let second_queue_id = second_queue.event_id;
    let admission = store
        .admit_turn_or_queue(
            session.session_id,
            session.settings_revision_id,
            &cancel_clear_event(&session, &branch),
            &[second_message, second_start],
            &[second_queue],
        )
        .expect("queue second turn");
    assert!(matches!(
        admission,
        SessionTurnAdmission::Queued { position: 1, .. }
    ));

    let metrics = store
        .load_session_inspection_metrics(session.session_id)
        .expect("metrics");
    assert_eq!(metrics.active_turn_count, 1);
    assert_eq!(metrics.turn_count, 1);
    assert_eq!(metrics.message_count, 1);

    store
        .append_event(&turn_finished_event(&session, &branch, first_turn_id))
        .expect("finish first turn");
    let continuation_turn_id = TurnId::new();
    let continuation_events = vec![
        EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::SessionQueuedMessageResolved {
                queue_event_id: second_queue_id,
                outcome: bt_core::QueuedMessageResolutionOutcome::Dispatched,
                reason: Some("test dispatch".to_owned()),
            },
        ),
        EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::User, "second"),
            },
        )
        .with_turn_id(continuation_turn_id),
        turn_started_event_with_source(
            &session,
            &branch,
            continuation_turn_id,
            TurnStartSource::QueuedFollowUp,
        ),
    ];
    let claim = store
        .claim_queued_continuation(session.session_id, second_queue_id, &continuation_events)
        .expect("claim queue");
    assert!(matches!(
        claim,
        ContinuationClaim::Claimed { ref seq_ids } if seq_ids.len() == 3
    ));

    store
        .append_event(&turn_finished_event(
            &session,
            &branch,
            continuation_turn_id,
        ))
        .expect("finish continuation");
    assert_eq!(
        store
            .claim_queued_continuation(session.session_id, second_queue_id, &continuation_events,)
            .expect("reject consumed queue"),
        ContinuationClaim::Stale
    );
}

#[test]
fn turn_transition_primitives_reject_malformed_event_batches() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let error = store
        .admit_turn_or_queue(
            session.session_id,
            session.settings_revision_id,
            &cancel_clear_event(&session, &branch),
            &[],
            &[queued_message_event(&session, &branch, "queued")],
        )
        .expect_err("empty started transition must fail");
    assert!(error.to_string().contains("turn transition"));
    assert!(
        store
            .load_all_events(session.session_id)
            .expect("events")
            .is_empty()
    );

    let queued = queued_message_event(&session, &branch, "queued");
    let queue_event_id = queued.event_id;
    store.append_event(&queued).expect("queue source");
    let error = store
        .claim_queued_continuation(session.session_id, queue_event_id, &[])
        .expect_err("empty continuation transition must fail");
    assert!(error.to_string().contains("turn transition"));
    assert_eq!(
        store
            .load_pending_queued_messages(session.session_id)
            .expect("pending queue")
            .len(),
        1
    );

    let turn_id = TurnId::new();
    let started_events = vec![
        EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::User, "new input"),
            },
        )
        .with_turn_id(turn_id),
        turn_started_event(&session, &branch, turn_id),
    ];
    let duplicate_queue_events = vec![
        queued_message_event(&session, &branch, "new input"),
        EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::SessionQueuedMessageEnqueued {
                message: Message::text(Role::User, "wrong revision"),
                settings_revision_id: session.settings_revision_id + 1,
            },
        ),
    ];
    let error = store
        .admit_turn_or_queue(
            session.session_id,
            session.settings_revision_id,
            &cancel_clear_event(&session, &branch),
            &started_events,
            &duplicate_queue_events,
        )
        .expect_err("duplicate queued transition must fail");
    assert!(error.to_string().contains("settings revision"));

    let continuation_turn_id = TurnId::new();
    let malformed_resolution = vec![
        EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::SessionQueuedMessageResolved {
                queue_event_id,
                outcome: bt_core::QueuedMessageResolutionOutcome::Dispatched,
                reason: None,
            },
        ),
        EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::SessionQueuedMessageResolved {
                queue_event_id: bt_core::EventId::new(),
                outcome: bt_core::QueuedMessageResolutionOutcome::Dropped,
                reason: Some("unexpected".to_owned()),
            },
        ),
        EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::User, "queued"),
            },
        )
        .with_turn_id(continuation_turn_id),
        turn_started_event_with_source(
            &session,
            &branch,
            continuation_turn_id,
            TurnStartSource::QueuedFollowUp,
        ),
    ];
    let error = store
        .claim_queued_continuation(session.session_id, queue_event_id, &malformed_resolution)
        .expect_err("mismatched extra resolution must fail");
    assert!(error.to_string().contains("unexpected source"));

    let wrong_revision_events = vec![
        EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::SessionQueuedMessageResolved {
                queue_event_id,
                outcome: bt_core::QueuedMessageResolutionOutcome::Dispatched,
                reason: None,
            },
        ),
        EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::User, "queued"),
            },
        )
        .with_turn_id(continuation_turn_id),
        turn_started_event_with_source_and_revision(
            &session,
            &branch,
            continuation_turn_id,
            TurnStartSource::QueuedFollowUp,
            session.settings_revision_id + 1,
        ),
    ];
    let error = store
        .claim_queued_continuation(session.session_id, queue_event_id, &wrong_revision_events)
        .expect_err("continuation must inherit its queued settings revision");
    assert!(error.to_string().contains("settings revision"));
    assert_eq!(
        store
            .load_pending_queued_messages(session.session_id)
            .expect("pending queue after malformed claims")
            .len(),
        1
    );
}

#[test]
fn steer_continuation_claim_is_scoped_to_one_branch() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    let child_branch = BranchRecord {
        branch_id: bt_core::BranchId::new(),
        session_id: session.session_id,
        parent_branch_id: Some(branch.branch_id),
        parent_event_id: None,
        head_event_id: None,
        summary: None,
        created_at: OffsetDateTime::now_utc(),
        is_default: false,
    };
    store
        .create_branch(&child_branch)
        .expect("create child branch");

    let root_steer = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::SessionSteered {
            message: "root steer".to_owned(),
            settings_revision_id: session.settings_revision_id,
        },
    );
    let root_steer_id = root_steer.event_id;
    store.append_event(&root_steer).expect("root steer");
    let child_steer = EventEnvelope::new(
        session.session_id,
        child_branch.branch_id,
        SpanKind::Agent,
        EventPayload::SessionSteered {
            message: "child steer".to_owned(),
            settings_revision_id: session.settings_revision_id,
        },
    );
    let child_steer_id = child_steer.event_id;
    store.append_event(&child_steer).expect("child steer");

    let turn_id = TurnId::new();
    let events = vec![
        EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::SessionSteersResolved {
                steer_event_ids: vec![root_steer_id],
                outcome: bt_core::SteerResolutionOutcome::Applied,
                combined_message: Some("root steer".to_owned()),
                reason: None,
            },
        ),
        EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::User, "root steer"),
            },
        )
        .with_turn_id(turn_id),
        turn_started_event_with_source(&session, &branch, turn_id, TurnStartSource::SteerFollowUp),
    ];
    let mut wrong_revision_events = events.clone();
    wrong_revision_events.pop();
    wrong_revision_events.push(turn_started_event_with_source_and_revision(
        &session,
        &branch,
        turn_id,
        TurnStartSource::SteerFollowUp,
        session.settings_revision_id + 1,
    ));
    let error = store
        .claim_steer_continuation(
            session.session_id,
            branch.branch_id,
            &[root_steer_id],
            &wrong_revision_events,
        )
        .expect_err("continuation must inherit the latest steer settings revision");
    assert!(error.to_string().contains("settings revision"));
    let claim = store
        .claim_steer_continuation(
            session.session_id,
            branch.branch_id,
            &[root_steer_id],
            &events,
        )
        .expect("claim root steer");
    assert!(matches!(claim, ContinuationClaim::Claimed { .. }));
    let pending = store
        .load_pending_steers(session.session_id)
        .expect("pending steers");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].steer_event_id, child_steer_id);
    assert_eq!(pending[0].branch_id, child_branch.branch_id);
}

#[test]
fn reopening_without_rebuild_marker_recovers_active_turn_projection() {
    let file = NamedTempFile::new().expect("tempfile");
    let (session, branch) = sample_session();
    let turn_id = TurnId::new();
    {
        let mut store = SqliteSessionStore::open(file.path()).expect("store");
        store
            .create_session(&session, &branch)
            .expect("create session");
        store
            .append_event(&turn_started_event(&session, &branch, turn_id))
            .expect("start turn");
    }
    {
        let connection = rusqlite::Connection::open(file.path()).expect("raw connection");
        connection
            .execute("DELETE FROM active_turn_projection", [])
            .expect("clear projection");
        connection
            .execute(
                "DELETE FROM migration_metadata WHERE key = 'active_turn_projection_v1_rebuilt'",
                [],
            )
            .expect("clear marker");
    }

    let mut store = SqliteSessionStore::open(file.path()).expect("reopen store");
    assert_eq!(
        store
            .load_session_inspection_metrics(session.session_id)
            .expect("metrics")
            .active_turn_count,
        1
    );
    let follow_up_turn_id = TurnId::new();
    let admission = store
        .admit_turn_or_queue(
            session.session_id,
            session.settings_revision_id,
            &cancel_clear_event(&session, &branch),
            &[
                EventEnvelope::new(
                    session.session_id,
                    branch.branch_id,
                    SpanKind::Agent,
                    EventPayload::MessageAppended {
                        message: Message::text(Role::User, "follow up"),
                    },
                )
                .with_turn_id(follow_up_turn_id),
                turn_started_event(&session, &branch, follow_up_turn_id),
            ],
            &[queued_message_event(&session, &branch, "follow up")],
        )
        .expect("queue behind reconstructed active turn");
    assert!(matches!(
        admission,
        SessionTurnAdmission::Queued { position: 1, .. }
    ));
}

#[test]
fn turn_admission_retries_stale_settings_without_appending_events() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    let turn_id = TurnId::new();
    let admission = store
        .admit_turn_or_queue(
            session.session_id,
            session.settings_revision_id + 1,
            &cancel_clear_event(&session, &branch),
            &[turn_started_event(&session, &branch, turn_id)],
            &[queued_message_event(&session, &branch, "queued")],
        )
        .expect("stale admission");
    assert_eq!(
        admission,
        SessionTurnAdmission::RetryWithSettings {
            settings_revision_id: session.settings_revision_id,
        }
    );
    assert!(
        !store
            .session_has_events(session.session_id)
            .expect("event presence")
    );
}

#[test]
fn exhausted_budget_blocks_direct_and_continuation_claims_without_consuming_work() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Session,
            EventPayload::BudgetConfigured {
                budget: BudgetConfig {
                    max_wall_clock_seconds: None,
                    max_tokens: None,
                    max_turns: Some(0),
                    max_cost_usd: None,
                },
            },
        ))
        .expect("configure exhausted budget");

    let direct_turn_id = TurnId::new();
    let admission = store
        .admit_turn_or_queue(
            session.session_id,
            session.settings_revision_id,
            &cancel_clear_event(&session, &branch),
            &[
                EventEnvelope::new(
                    session.session_id,
                    branch.branch_id,
                    SpanKind::Agent,
                    EventPayload::MessageAppended {
                        message: Message::text(Role::User, "blocked direct work"),
                    },
                )
                .with_turn_id(direct_turn_id),
                turn_started_event(&session, &branch, direct_turn_id),
            ],
            &[queued_message_event(
                &session,
                &branch,
                "blocked direct work",
            )],
        )
        .expect("evaluate direct admission");
    assert_eq!(admission, SessionTurnAdmission::BudgetExhausted);

    let queued = queued_message_event(&session, &branch, "queued work");
    let queue_event_id = queued.event_id;
    store.append_event(&queued).expect("queue work");
    let queued_turn_id = TurnId::new();
    let queued_claim = store
        .claim_queued_continuation(
            session.session_id,
            queue_event_id,
            &[
                EventEnvelope::new(
                    session.session_id,
                    branch.branch_id,
                    SpanKind::Agent,
                    EventPayload::SessionQueuedMessageResolved {
                        queue_event_id,
                        outcome: bt_core::QueuedMessageResolutionOutcome::Dispatched,
                        reason: Some("test dispatch".to_owned()),
                    },
                ),
                EventEnvelope::new(
                    session.session_id,
                    branch.branch_id,
                    SpanKind::Agent,
                    EventPayload::MessageAppended {
                        message: Message::text(Role::User, "queued work"),
                    },
                )
                .with_turn_id(queued_turn_id),
                turn_started_event_with_source(
                    &session,
                    &branch,
                    queued_turn_id,
                    TurnStartSource::QueuedFollowUp,
                ),
            ],
        )
        .expect("evaluate queue claim");
    assert_eq!(queued_claim, ContinuationClaim::BudgetExhausted);

    let steer = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::SessionSteered {
            message: "steered work".to_owned(),
            settings_revision_id: session.settings_revision_id,
        },
    );
    let steer_event_id = steer.event_id;
    store.append_event(&steer).expect("steer work");
    let steer_turn_id = TurnId::new();
    let steer_claim = store
        .claim_steer_continuation(
            session.session_id,
            branch.branch_id,
            &[steer_event_id],
            &[
                EventEnvelope::new(
                    session.session_id,
                    branch.branch_id,
                    SpanKind::Agent,
                    EventPayload::SessionSteersResolved {
                        steer_event_ids: vec![steer_event_id],
                        outcome: bt_core::SteerResolutionOutcome::Applied,
                        combined_message: Some("steered work".to_owned()),
                        reason: None,
                    },
                ),
                EventEnvelope::new(
                    session.session_id,
                    branch.branch_id,
                    SpanKind::Agent,
                    EventPayload::MessageAppended {
                        message: Message::text(Role::User, "steered work"),
                    },
                )
                .with_turn_id(steer_turn_id),
                turn_started_event_with_source(
                    &session,
                    &branch,
                    steer_turn_id,
                    TurnStartSource::SteerFollowUp,
                ),
            ],
        )
        .expect("evaluate steer claim");
    assert_eq!(steer_claim, ContinuationClaim::BudgetExhausted);

    assert_eq!(
        store
            .load_pending_queued_messages(session.session_id)
            .expect("pending queue")
            .len(),
        1
    );
    assert_eq!(
        store
            .load_pending_steers(session.session_id)
            .expect("pending steers")
            .len(),
        1
    );
    assert_eq!(
        store
            .load_session_inspection_metrics(session.session_id)
            .expect("metrics")
            .active_turn_count,
        0
    );
}

#[test]
fn committed_settings_updates_allocate_immutable_revisions() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let first = store
        .commit_session_settings_update(
            session.session_id,
            branch.branch_id,
            Some(ConnectionId::new("remote")),
            Some(Some("model-a".to_owned())),
            None,
        )
        .expect("first settings update");
    assert_eq!(first.session.settings_revision_id, 2);
    assert_eq!(first.event.seq_id, None);

    let conflicting = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Session,
        EventPayload::SessionSettingsUpdated {
            settings_revision_id: 2,
            connection_id: "different".to_owned(),
            model_id: Some("different-model".to_owned()),
            tool_mode: "minimal".to_owned(),
        },
    );
    let error = store
        .append_event(&conflicting)
        .expect_err("revision snapshots are immutable");
    assert!(error.to_string().contains("immutable"));

    let snapshot = store
        .load_session_settings_revision(session.session_id, 2)
        .expect("load revision")
        .expect("revision exists");
    assert_eq!(snapshot.connection_id, ConnectionId::new("remote"));
    assert_eq!(snapshot.model_id.as_deref(), Some("model-a"));
    assert_eq!(snapshot.tool_mode, SessionToolMode::Extended);

    let second = store
        .commit_session_settings_update(
            session.session_id,
            branch.branch_id,
            None,
            Some(Some("model-b".to_owned())),
            Some(SessionToolMode::Standard),
        )
        .expect("second settings update");
    assert_eq!(second.session.settings_revision_id, 3);
    assert_eq!(second.session.connection_id, ConnectionId::new("remote"));
    assert_eq!(second.session.model_id.as_deref(), Some("model-b"));
    assert_eq!(second.session.tool_mode, SessionToolMode::Standard);

    let identical_revision_replay = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Session,
        first.event.payload.clone(),
    );
    store
        .append_event(&identical_revision_replay)
        .expect("identical historical revision replay");
    let current = store
        .load_session(session.session_id)
        .expect("load current session")
        .expect("session exists");
    assert_eq!(current.settings_revision_id, 3);
    assert_eq!(current.model_id.as_deref(), Some("model-b"));
    let replayed = store
        .load_session_settings_revision(session.session_id, 2)
        .expect("load replayed revision")
        .expect("replayed revision exists");
    assert_eq!(replayed.connection_id, ConnectionId::new("remote"));
    assert_eq!(replayed.model_id.as_deref(), Some("model-a"));
    assert_eq!(replayed.tool_mode, SessionToolMode::Extended);
}

#[test]
fn stale_turn_finish_does_not_clear_newer_active_turn() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    let first_turn_id = TurnId::new();
    let second_turn_id = TurnId::new();
    store
        .append_event(&turn_started_event(&session, &branch, first_turn_id))
        .expect("start first");
    store
        .append_event(&turn_finished_event(&session, &branch, first_turn_id))
        .expect("finish first");
    store
        .append_event(&turn_started_event(&session, &branch, second_turn_id))
        .expect("start second");
    store
        .append_event(&turn_finished_event(&session, &branch, first_turn_id))
        .expect("append stale finish");
    assert_eq!(
        store
            .load_session_inspection_metrics(session.session_id)
            .expect("metrics")
            .active_turn_count,
        1
    );
}

#[test]
fn cancel_request_blocks_queued_continuation_claim() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    let queued = queued_message_event(&session, &branch, "queued");
    let queue_event_id = queued.event_id;
    store.append_event(&queued).expect("queue message");
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::SessionCancelled {
                reason: "operator cancelled".to_owned(),
            },
        ))
        .expect("cancel");
    assert_eq!(
        store
            .claim_queued_continuation(session.session_id, queue_event_id, &[])
            .expect("cancel wins"),
        ContinuationClaim::CancelPending
    );
}

#[test]
fn set_default_branch_with_event_rejects_missing_branch_without_recording_event() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    let missing_branch_id = bt_core::BranchId::new();
    let event = EventEnvelope::new(
        session.session_id,
        missing_branch_id,
        SpanKind::Chain,
        EventPayload::BranchActivated {
            branch_id: missing_branch_id,
        },
    );

    let error = store
        .set_default_branch_with_event(session.session_id, missing_branch_id, &event)
        .expect_err("missing branch should fail closed");
    assert!(error.to_string().contains("not found"));
    assert!(
        store
            .load_all_events(session.session_id)
            .expect("events")
            .is_empty()
    );
    let branches = store.load_branches(session.session_id).expect("branches");
    assert_eq!(branches.len(), 1);
    assert_eq!(branches[0].branch_id, branch.branch_id);
    assert!(branches[0].is_default);
}

#[test]
fn context_manifest_projection_preserves_model_visible_message_sources() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let message = Message::text(Role::User, "summarize this project");
    let message_seq_id = store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: message.clone(),
            },
        ))
        .expect("append message");
    let request = CompletionRequest {
        connection_id: session.connection_id.clone(),
        model: "qwen3.5:latest".to_owned(),
        system_prompt: Some("system".to_owned()),
        messages: vec![message],
        tools: Vec::new(),
        structured_output: None,
        max_tokens: None,
        temperature: None,
        thinking: None,
    };
    let turn_id = TurnId::new();
    let manifest = ContextManifest::from_completion_request(
        turn_id,
        branch.branch_id,
        1,
        "openai-compatible",
        request.model.clone(),
        session.settings_revision_id,
        Some(message_seq_id),
        &request,
        false,
        &[ContextMessageSourceRef {
            message_id: request.messages[0].message_id,
            branch_id: branch.branch_id,
            seq_id: message_seq_id,
        }],
    );
    let manifest_seq_id = store
        .append_event(
            &EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Llm,
                EventPayload::TurnContextManifestRecorded {
                    manifest: manifest.clone(),
                },
            )
            .with_turn_id(turn_id),
        )
        .expect("append manifest");

    let projections = store
        .load_turn_context_manifests(session.session_id, turn_id)
        .expect("load manifests");
    assert_eq!(projections.len(), 1);
    let projection = &projections[0];
    assert_eq!(projection.session_id, session.session_id);
    assert_eq!(projection.branch_id, branch.branch_id);
    assert_eq!(projection.llm_call_ordinal, 1);
    assert_eq!(projection.recorded_seq_id, manifest_seq_id);
    assert_eq!(projection.message_count, 1);
    assert_eq!(projection.tool_count, 0);
    assert_eq!(projection.attachment_count, 0);
    assert_eq!(projection.context_boundary_seq_id, Some(message_seq_id));
    assert_eq!(projection.manifest, manifest);
    assert_eq!(
        projection.manifest.messages[0].source_branch_id,
        Some(branch.branch_id)
    );
    assert_eq!(
        projection.manifest.messages[0].source_seq_id,
        Some(message_seq_id)
    );
}

#[test]
fn inspection_metrics_capture_counts_and_turn_boundaries() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let turn_id = bt_core::TurnId::new();
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::User, "hello"),
            },
        ))
        .expect("append user message");
    store
        .append_event(
            &EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Agent,
                EventPayload::TurnStarted {
                    turn_id,
                    provider: "test".to_owned(),
                    model: "demo".to_owned(),
                    message_count: 1,
                    settings_revision_id: default_settings_revision_id(),
                    source: bt_core::TurnStartSource::UserMessage,
                    resumed_from_call_id: None,
                },
            )
            .with_turn_id(turn_id),
        )
        .expect("append turn started");
    store
        .append_event(
            &EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Tool,
                EventPayload::ToolApprovalRequested {
                    call_id: ToolCallId::new("call-1"),
                    tool_name: "shell".to_owned(),
                    snapshot: None,
                },
            )
            .with_turn_id(turn_id),
        )
        .expect("append approval request");
    store
        .append_raw_chunk(
            session.session_id,
            branch.branch_id,
            Some(turn_id),
            None,
            None,
            "test",
            "chat",
            b"chunk",
        )
        .expect("append raw chunk");

    let metrics = store
        .load_session_inspection_metrics(session.session_id)
        .expect("inspection metrics");
    assert_eq!(metrics.message_count, 1);
    assert_eq!(metrics.turn_count, 1);
    assert_eq!(metrics.approval_count, 1);
    assert_eq!(metrics.pending_approval_count, 1);
    assert_eq!(metrics.raw_chunk_count, 1);
    assert_eq!(metrics.active_turn_count, 1);
}

#[test]
fn approval_projection_preserves_request_snapshot_and_resolution() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let turn_id = TurnId::new();
    let call_id = ToolCallId::new("call-approval-snapshot");
    let requested_at = OffsetDateTime::UNIX_EPOCH;
    let request = bt_core::ApprovalRequest {
        session_id: session.session_id,
        call_id: call_id.clone(),
        tool_name: "shell".to_owned(),
        arguments: serde_json::json!({
            "command": "echo safe",
            "api_key": "should-not-leak"
        }),
        requirement: bt_core::ApprovalRequirement::Always,
        tool_metadata: bt_core::ToolMetadata {
            risk_class: bt_core::ToolRiskClass::High,
            is_read_only: false,
            is_concurrency_safe: false,
            interrupt_behavior: bt_core::ToolInterruptBehavior::TerminateProcess,
            execution_mode: bt_core::ToolExecutionMode::Immediate,
            should_defer: false,
            catalogue_tags: vec!["shell".to_owned()],
            display_group: bt_core::ToolDisplayGroup::Execution,
        },
        requested_at,
    };
    let snapshot = request.snapshot(
        bt_core::ToolOperationInitiator::Agent,
        "agent_turn",
        Some("registry:test".to_owned()),
        Some("requires_approval:shell".to_owned()),
    );
    let decision = bt_core::ApprovalDecision::Approved {
        decided_at: requested_at,
        decided_by: "bt-tui".to_owned(),
        scope: bt_core::ApprovalScope::Session,
        source: bt_core::ApprovalDecisionSource::Human,
    };
    let resolution = bt_core::ApprovalResolution::from_request(&request, decision.clone());

    store
        .append_event(
            &EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Tool,
                EventPayload::ToolApprovalRequested {
                    call_id: call_id.clone(),
                    tool_name: request.tool_name.clone(),
                    snapshot: Some(snapshot.clone()),
                },
            )
            .with_turn_id(turn_id),
        )
        .expect("append approval request");
    store
        .append_event(
            &EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Tool,
                EventPayload::ToolApprovalResolved {
                    call_id: call_id.clone(),
                    tool_name: request.tool_name.clone(),
                    request_fingerprint: None,
                    resolution: Some(resolution.clone()),
                    decision,
                },
            )
            .with_turn_id(turn_id),
        )
        .expect("append approval resolution");

    let approvals = store
        .load_approvals(session.session_id)
        .expect("load approvals");
    assert_eq!(approvals.len(), 1);
    let approval = &approvals[0];
    assert_eq!(
        approval.request_fingerprint.as_deref(),
        Some(snapshot.request_fingerprint.as_str())
    );
    assert_eq!(approval.request_snapshot.as_ref(), Some(&snapshot));
    assert_eq!(approval.resolution.as_ref(), Some(&resolution));
    assert_eq!(
        approval
            .request_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.redacted_arguments_preview.as_ref())
            .and_then(|preview| preview.get("api_key"))
            .and_then(|value| value.as_str()),
        Some("[redacted]")
    );
}

#[test]
fn tool_and_approval_projections_isolate_equal_call_ids_by_session() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (first_session, first_branch) = sample_session();
    let (second_session, second_branch) = sample_session();
    store
        .create_session(&first_session, &first_branch)
        .expect("create first session");
    store
        .create_session(&second_session, &second_branch)
        .expect("create second session");

    let call_id = ToolCallId::new("provider-call-id");
    for (session, branch, tool_name, decision) in [
        (
            &first_session,
            &first_branch,
            "first-tool",
            ApprovalDecision::Approved {
                decided_at: OffsetDateTime::UNIX_EPOCH,
                decided_by: "first-operator".to_owned(),
                scope: ApprovalScope::Session,
                source: ApprovalDecisionSource::Human,
            },
        ),
        (
            &second_session,
            &second_branch,
            "second-tool",
            ApprovalDecision::Denied {
                decided_at: OffsetDateTime::UNIX_EPOCH,
                decided_by: "second-operator".to_owned(),
                reason: Some("different session".to_owned()),
                scope: ApprovalScope::Session,
                source: ApprovalDecisionSource::Human,
            },
        ),
    ] {
        store
            .append_event(&EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Tool,
                EventPayload::ToolCallRequested {
                    call_id: call_id.clone(),
                    tool_name: tool_name.to_owned(),
                    arguments: serde_json::json!({ "session": session.session_id.to_string() }),
                },
            ))
            .expect("append tool request");
        store
            .append_event(&EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Tool,
                EventPayload::ToolApprovalRequested {
                    call_id: call_id.clone(),
                    tool_name: tool_name.to_owned(),
                    snapshot: None,
                },
            ))
            .expect("append approval request");
        store
            .append_event(&EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Tool,
                EventPayload::ToolApprovalResolved {
                    call_id: call_id.clone(),
                    tool_name: tool_name.to_owned(),
                    request_fingerprint: None,
                    resolution: None,
                    decision,
                },
            ))
            .expect("append approval resolution");
    }

    let first_tool_runs = store
        .load_tool_runs(first_session.session_id)
        .expect("load first tool runs");
    let second_tool_runs = store
        .load_tool_runs(second_session.session_id)
        .expect("load second tool runs");
    assert_eq!(first_tool_runs.len(), 1);
    assert_eq!(second_tool_runs.len(), 1);
    assert_eq!(first_tool_runs[0].tool_name, "first-tool");
    assert_eq!(second_tool_runs[0].tool_name, "second-tool");
    assert_eq!(
        first_tool_runs[0].arguments,
        Some(serde_json::json!({ "session": first_session.session_id.to_string() }))
    );
    assert_eq!(
        second_tool_runs[0].arguments,
        Some(serde_json::json!({ "session": second_session.session_id.to_string() }))
    );

    let first_approvals = store
        .load_approvals(first_session.session_id)
        .expect("load first approvals");
    let second_approvals = store
        .load_approvals(second_session.session_id)
        .expect("load second approvals");
    assert_eq!(first_approvals.len(), 1);
    assert_eq!(second_approvals.len(), 1);
    assert_eq!(first_approvals[0].tool_name, "first-tool");
    assert_eq!(first_approvals[0].status, "approved");
    assert_eq!(second_approvals[0].tool_name, "second-tool");
    assert_eq!(second_approvals[0].status, "denied");
}

#[test]
fn reused_call_id_projects_only_the_latest_request_instance() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    let call_id = ToolCallId::new("reused-call-id");

    for payload in [
        EventPayload::ToolCallRequested {
            call_id: call_id.clone(),
            tool_name: "shell".to_owned(),
            arguments: serde_json::json!({"command":"pwd"}),
        },
        EventPayload::ToolApprovalRequested {
            call_id: call_id.clone(),
            tool_name: "shell".to_owned(),
            snapshot: None,
        },
        EventPayload::ToolApprovalResolved {
            call_id: call_id.clone(),
            tool_name: "shell".to_owned(),
            request_fingerprint: Some("first-request".to_owned()),
            resolution: None,
            decision: ApprovalDecision::Approved {
                decided_at: OffsetDateTime::UNIX_EPOCH,
                decided_by: "operator".to_owned(),
                scope: ApprovalScope::Once,
                source: ApprovalDecisionSource::Human,
            },
        },
        EventPayload::ToolExecutionFinished {
            call_id: call_id.clone(),
            tool_name: "shell".to_owned(),
            result: ToolResultEnvelope {
                call_id: call_id.clone(),
                tool_name: "shell".to_owned(),
                is_error: false,
                output: serde_json::json!({"stdout":"first"}),
                duration_ms: Some(1),
            },
        },
        EventPayload::ToolCallRequested {
            call_id: call_id.clone(),
            tool_name: "shell".to_owned(),
            arguments: serde_json::json!({"command":"ls"}),
        },
        EventPayload::ToolApprovalRequested {
            call_id: call_id.clone(),
            tool_name: "shell".to_owned(),
            snapshot: None,
        },
    ] {
        store
            .append_event(&EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Tool,
                payload,
            ))
            .expect("append lifecycle event");
    }

    let tool_run = store
        .load_tool_runs(session.session_id)
        .expect("load tool runs")
        .pop()
        .expect("tool run");
    assert_eq!(tool_run.status, "requested");
    assert_eq!(
        tool_run.arguments,
        Some(serde_json::json!({"command":"ls"}))
    );
    assert!(tool_run.result.is_none());

    let approval = store
        .load_approvals(session.session_id)
        .expect("load approvals")
        .pop()
        .expect("approval");
    assert_eq!(approval.status, "pending");
    assert!(approval.decision.is_none());
    assert!(approval.resolution.is_none());
}

#[test]
fn stale_resume_cannot_claim_a_newer_request_with_the_same_call_id() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    let call_id = ToolCallId::new("reused-call-id");
    let mut request_sequences = Vec::new();
    let request_turns = [TurnId::new(), TurnId::new()];

    for (turn_id, command) in request_turns.into_iter().zip(["pwd", "ls"]) {
        store
            .append_event(&turn_started_event(&session, &branch, turn_id))
            .expect("start request turn");
        store
            .append_event(
                &EventEnvelope::new(
                    session.session_id,
                    branch.branch_id,
                    SpanKind::Tool,
                    EventPayload::ToolCallRequested {
                        call_id: call_id.clone(),
                        tool_name: "shell".to_owned(),
                        arguments: serde_json::json!({"command": command}),
                    },
                )
                .with_turn_id(turn_id),
            )
            .expect("append tool request");
        request_sequences.push(
            store
                .append_event(
                    &EventEnvelope::new(
                        session.session_id,
                        branch.branch_id,
                        SpanKind::Tool,
                        EventPayload::ToolApprovalRequested {
                            call_id: call_id.clone(),
                            tool_name: "shell".to_owned(),
                            snapshot: None,
                        },
                    )
                    .with_turn_id(turn_id),
                )
                .expect("append approval request"),
        );
        store
            .append_event(&turn_finished_event(&session, &branch, turn_id))
            .expect("finish request turn");
    }

    let approvals = store
        .load_approvals(session.session_id)
        .expect("load approvals");
    let latest = approvals.first().expect("latest approval");
    assert_eq!(latest.turn_id, Some(request_turns[1]));
    assert_eq!(latest.requested_seq_id, Some(request_sequences[1]));
    let event_count = store
        .load_all_events(session.session_id)
        .expect("events before stale claim")
        .len();
    let resumed_turn_id = TurnId::new();
    let resume_events = vec![
        EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::TurnStarted {
                turn_id: resumed_turn_id,
                provider: "test".to_owned(),
                model: "test-model".to_owned(),
                message_count: 1,
                settings_revision_id: session.settings_revision_id,
                source: TurnStartSource::ApprovalResume,
                resumed_from_call_id: Some(call_id.clone()),
            },
        )
        .with_turn_id(resumed_turn_id),
        EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Tool,
            EventPayload::ToolApprovalResolved {
                call_id: call_id.clone(),
                tool_name: "shell".to_owned(),
                request_fingerprint: None,
                resolution: None,
                decision: ApprovalDecision::Approved {
                    decided_at: OffsetDateTime::now_utc(),
                    decided_by: "operator".to_owned(),
                    scope: ApprovalScope::Once,
                    source: ApprovalDecisionSource::Human,
                },
            },
        )
        .with_turn_id(resumed_turn_id),
    ];

    let outcome = store
        .claim_resumed_continuation(
            session.session_id,
            branch.branch_id,
            &call_id,
            "shell",
            request_turns[0],
            request_sequences[0],
            session.settings_revision_id,
            ResumedContinuationKind::Approval,
            &resume_events,
        )
        .expect("stale claim result");
    assert_eq!(outcome, ContinuationClaim::Stale);
    assert_eq!(
        store
            .load_all_events(session.session_id)
            .expect("events after stale claim")
            .len(),
        event_count
    );
    let latest = store
        .load_approvals(session.session_id)
        .expect("load approval after claim")
        .pop()
        .expect("latest approval remains");
    assert_eq!(latest.status, "pending");
    assert_eq!(latest.requested_seq_id, Some(request_sequences[1]));
}

#[test]
fn migration_rebuilds_tool_and_approval_projections_with_composite_identity() {
    let file = NamedTempFile::new().expect("tempfile");
    let (first_session, first_branch) = sample_session();
    let (second_session, second_branch) = sample_session();
    let call_id = ToolCallId::new("reused-provider-call-id");

    {
        let mut store = SqliteSessionStore::open(file.path()).expect("open store");
        for (session, branch, tool_name) in [
            (&first_session, &first_branch, "first-tool"),
            (&second_session, &second_branch, "second-tool"),
        ] {
            store
                .create_session(session, branch)
                .expect("create session");
            store
                .append_event(&EventEnvelope::new(
                    session.session_id,
                    branch.branch_id,
                    SpanKind::Tool,
                    EventPayload::ToolCallRequested {
                        call_id: call_id.clone(),
                        tool_name: tool_name.to_owned(),
                        arguments: serde_json::json!({ "tool": tool_name }),
                    },
                ))
                .expect("append tool request");
            store
                .append_event(&EventEnvelope::new(
                    session.session_id,
                    branch.branch_id,
                    SpanKind::Tool,
                    EventPayload::ToolApprovalRequested {
                        call_id: call_id.clone(),
                        tool_name: tool_name.to_owned(),
                        snapshot: None,
                    },
                ))
                .expect("append approval request");
        }

        store
            .connection
            .execute_batch(
                "DROP TABLE approval_projection;
                 DROP TABLE tool_run_projection;
                 CREATE TABLE approval_projection (
                    call_id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    tool_name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    request_fingerprint TEXT,
                    request_snapshot_json TEXT,
                    decision_json TEXT,
                    resolution_json TEXT,
                    updated_at TEXT NOT NULL
                 );
                 CREATE TABLE tool_run_projection (
                    call_id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    tool_name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    arguments_json TEXT,
                    result_json TEXT,
                    updated_at TEXT NOT NULL
                 );",
            )
            .expect("replace with legacy projection schema");
    }

    let store = SqliteSessionStore::open(file.path()).expect("migrate store");
    for (session, tool_name) in [
        (&first_session, "first-tool"),
        (&second_session, "second-tool"),
    ] {
        let tool_runs = store
            .load_tool_runs(session.session_id)
            .expect("load rebuilt tool runs");
        let approvals = store
            .load_approvals(session.session_id)
            .expect("load rebuilt approvals");
        assert_eq!(tool_runs.len(), 1);
        assert_eq!(tool_runs[0].tool_name, tool_name);
        assert_eq!(approvals.len(), 1);
        assert_eq!(approvals[0].tool_name, tool_name);
    }

    for table in ["approval_projection", "tool_run_projection"] {
        let mut statement = store
            .connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .expect("table info");
        let primary_key_columns = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
            })
            .expect("primary key columns")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect primary key columns");
        assert!(primary_key_columns.contains(&("session_id".to_owned(), 1)));
        assert!(primary_key_columns.contains(&("call_id".to_owned(), 2)));
        for identity_column in ["branch_id", "turn_id", "requested_seq_id"] {
            assert!(
                primary_key_columns
                    .iter()
                    .any(|(column, _)| column == identity_column),
                "{table} is missing {identity_column}"
            );
        }
    }
}

#[test]
fn unfinished_resumed_turn_recovery_scans_turn_boundaries_only() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (user_session, user_branch) = sample_session();
    let (finished_session, finished_branch) = sample_session();
    let (unfinished_session, unfinished_branch) = sample_session();
    for (session, branch) in [
        (&user_session, &user_branch),
        (&finished_session, &finished_branch),
        (&unfinished_session, &unfinished_branch),
    ] {
        store
            .create_session(session, branch)
            .expect("create session");
    }

    let user_turn_id = TurnId::new();
    let finished_resume_id = TurnId::new();
    let unfinished_resume_id = TurnId::new();

    for (session, branch, turn_id, source) in [
        (
            &user_session,
            &user_branch,
            user_turn_id,
            TurnStartSource::UserMessage,
        ),
        (
            &finished_session,
            &finished_branch,
            finished_resume_id,
            TurnStartSource::ApprovalResume,
        ),
        (
            &unfinished_session,
            &unfinished_branch,
            unfinished_resume_id,
            TurnStartSource::InputResume,
        ),
    ] {
        store
            .append_event(
                &EventEnvelope::new(
                    session.session_id,
                    branch.branch_id,
                    SpanKind::Agent,
                    EventPayload::TurnStarted {
                        turn_id,
                        provider: "test-provider".to_owned(),
                        model: "test-model".to_owned(),
                        message_count: 1,
                        settings_revision_id: default_settings_revision_id(),
                        source,
                        resumed_from_call_id: None,
                    },
                )
                .with_turn_id(turn_id),
            )
            .expect("append turn start");
    }

    store
        .append_event(
            &EventEnvelope::new(
                finished_session.session_id,
                finished_branch.branch_id,
                SpanKind::Agent,
                EventPayload::TurnFinished {
                    turn_id: finished_resume_id,
                    provider: "test-provider".to_owned(),
                    model: "test-model".to_owned(),
                    status: "completed".to_owned(),
                    finish_reason: Some("done".to_owned()),
                    latency_ms: 1,
                },
            )
            .with_turn_id(finished_resume_id),
        )
        .expect("append finished resumed turn");

    let unfinished = store
        .load_unfinished_resumed_turns()
        .expect("unfinished resumed turns");
    assert_eq!(unfinished.len(), 1);
    assert_eq!(unfinished[0].session_id, unfinished_session.session_id);
    assert_eq!(unfinished[0].branch_id, unfinished_branch.branch_id);
    assert_eq!(unfinished[0].turn_id, unfinished_resume_id);
    assert_eq!(unfinished[0].provider, "test-provider");
    assert_eq!(unfinished[0].model, "test-model");
}

#[test]
fn unfinished_turn_recovery_includes_user_message_turns() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (user_session, user_branch) = sample_session();
    let (finished_session, finished_branch) = sample_session();
    for (session, branch) in [
        (&user_session, &user_branch),
        (&finished_session, &finished_branch),
    ] {
        store
            .create_session(session, branch)
            .expect("create session");
    }

    let user_turn_id = TurnId::new();
    let finished_resume_id = TurnId::new();
    for (session, branch, turn_id, source) in [
        (
            &user_session,
            &user_branch,
            user_turn_id,
            TurnStartSource::UserMessage,
        ),
        (
            &finished_session,
            &finished_branch,
            finished_resume_id,
            TurnStartSource::ApprovalResume,
        ),
    ] {
        store
            .append_event(
                &EventEnvelope::new(
                    session.session_id,
                    branch.branch_id,
                    SpanKind::Agent,
                    EventPayload::TurnStarted {
                        turn_id,
                        provider: "test-provider".to_owned(),
                        model: "test-model".to_owned(),
                        message_count: 1,
                        settings_revision_id: default_settings_revision_id(),
                        source,
                        resumed_from_call_id: None,
                    },
                )
                .with_turn_id(turn_id),
            )
            .expect("append turn start");
    }

    store
        .append_event(
            &EventEnvelope::new(
                finished_session.session_id,
                finished_branch.branch_id,
                SpanKind::Agent,
                EventPayload::TurnFinished {
                    turn_id: finished_resume_id,
                    provider: "test-provider".to_owned(),
                    model: "test-model".to_owned(),
                    status: "completed".to_owned(),
                    finish_reason: Some("done".to_owned()),
                    latency_ms: 1,
                },
            )
            .with_turn_id(finished_resume_id),
        )
        .expect("append finished resumed turn");

    let unfinished = store.load_unfinished_turns().expect("unfinished turns");
    assert_eq!(unfinished.len(), 1);
    assert_eq!(unfinished[0].turn_id, user_turn_id);
    assert_eq!(unfinished[0].source, TurnStartSource::UserMessage);
}

#[test]
fn event_log_reconstructs_turn_instruction_provenance() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let turn_id = TurnId::new();
    let core_prompt = InstructionDocument {
        source: "data/prompts/00-core.md".to_owned(),
        title: "00-core".to_owned(),
        body: "Core harness prompt.".to_owned(),
    };
    let provider_overlay = Some(InstructionDocument {
        source: "data/prompts/provider/openai-compatible.md".to_owned(),
        title: "openai-compatible".to_owned(),
        body: "Provider guidance.".to_owned(),
    });
    let instructions = vec![
        InstructionDocument {
            source: "/tmp/global/10-global.md".to_owned(),
            title: "10-global".to_owned(),
            body: "Global skill.".to_owned(),
        },
        InstructionDocument {
            source: "/tmp/project/.belltower/skills/20-project.md".to_owned(),
            title: "20-project".to_owned(),
            body: "Project skill.".to_owned(),
        },
    ];
    let provenance = TurnInstructionProvenance {
        turn_id,
        provider: "openai-compatible".to_owned(),
        model: "demo".to_owned(),
        settings_revision_id: default_settings_revision_id(),
        core_prompt,
        provider_overlay,
        instructions,
        rendered_system_prompt:
            "Core harness prompt.\n\nRuntime context:\n- cwd: /tmp/project\n\nGlobal skill.\n\nProject skill."
                .to_owned(),
    };

    store
        .append_event(
            &EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Agent,
                EventPayload::TurnStarted {
                    turn_id,
                    provider: "openai-compatible".to_owned(),
                    model: "demo".to_owned(),
                    message_count: 1,
                    settings_revision_id: default_settings_revision_id(),
                    source: bt_core::TurnStartSource::UserMessage,
                    resumed_from_call_id: None,
                },
            )
            .with_turn_id(turn_id),
        )
        .expect("append turn started");
    store
        .append_event(
            &EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Agent,
                EventPayload::TurnInstructionProvenanceRecorded {
                    provenance: provenance.clone(),
                },
            )
            .with_turn_id(turn_id),
        )
        .expect("append provenance");

    let events = store
        .load_session_event_log(session.session_id)
        .expect("load event log");
    let reconstructed = instruction_provenance_for_turn(&events, turn_id).expect("turn provenance");

    assert_eq!(reconstructed, provenance);
}

#[test]
fn message_projection_and_replay_work() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let event = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::MessageAppended {
            message: Message::text(Role::User, "hello"),
        },
    );

    let seq_id = store.append_event(&event).expect("append event");
    assert_eq!(seq_id, 1);

    let events = store
        .load_events_after(session.session_id, None, 100)
        .expect("load events");
    assert_eq!(events.len(), 1);

    let messages = store
        .load_messages(session.session_id, Some(branch.branch_id))
        .expect("load messages");
    assert_eq!(messages.len(), 1);
}

#[test]
fn completion_updates_cost_projection() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let event = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Llm,
        EventPayload::CompletionFinished {
            llm_call_ordinal: 1,
            provider: "openai-compatible".to_owned(),
            model: "o4-mini".to_owned(),
            usage: bt_core::TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                cache_read_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
            },
            cost: Some(bt_core::CostBreakdown {
                prompt_usd: 0.01,
                completion_usd: 0.02,
                total_usd: 0.03,
                cache_read_usd: None,
                cache_write_usd: None,
                reasoning_usd: None,
            }),
            finish_reason: "stop".to_owned(),
            latency_ms: 42,
        },
    );

    store.append_event(&event).expect("append event");
    let projection = store
        .load_cost_summary(session.session_id)
        .expect("projection lookup")
        .expect("projection exists");
    assert_eq!(projection.total_tokens, 15);
    assert_eq!(projection.total_cost_usd, 0.03);
    assert_eq!(projection.unpriced_completion_count, 0);
}

#[test]
fn unpriced_completion_updates_counter_without_cost() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let event = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Llm,
        EventPayload::CompletionFinished {
            llm_call_ordinal: 1,
            provider: "openai-compatible".to_owned(),
            model: "unpriced-model".to_owned(),
            usage: bt_core::TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                cache_read_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
            },
            cost: None,
            finish_reason: "stop".to_owned(),
            latency_ms: 42,
        },
    );

    store.append_event(&event).expect("append event");
    let projection = store
        .load_cost_summary(session.session_id)
        .expect("projection lookup")
        .expect("projection exists");
    assert_eq!(projection.prompt_tokens, 10);
    assert_eq!(projection.completion_tokens, 5);
    assert_eq!(projection.total_tokens, 15);
    assert_eq!(projection.total_cost_usd, 0.0);
    assert_eq!(projection.unpriced_completion_count, 1);
}

#[test]
fn reprojection_repairs_cost_summary_without_mutating_events() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let event = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Llm,
        EventPayload::CompletionFinished {
            llm_call_ordinal: 1,
            provider: "openai-compatible".to_owned(),
            model: "newly-priced-model".to_owned(),
            usage: bt_core::TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
                cache_read_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
            },
            cost: None,
            finish_reason: "stop".to_owned(),
            latency_ms: 42,
        },
    );

    store.append_event(&event).expect("append event");
    let before = store
        .load_cost_summary(session.session_id)
        .expect("projection lookup")
        .expect("projection exists");
    assert_eq!(before.total_cost_usd, 0.0);
    assert_eq!(before.unpriced_completion_count, 1);

    let report = store
        .reproject_completion_costs(
            session.session_id,
            |provider: &str,
             model: &str,
             usage: &bt_core::TokenUsage,
             existing_cost: Option<&bt_core::CostBreakdown>|
             -> bt_core::Result<Option<bt_core::CostBreakdown>> {
                assert_eq!(provider, "openai-compatible");
                assert_eq!(model, "newly-priced-model");
                assert_eq!(usage.total_tokens, 150);
                assert!(existing_cost.is_none());
                Ok(Some(bt_core::CostBreakdown {
                    prompt_usd: 0.10,
                    completion_usd: 0.25,
                    total_usd: 0.35,
                    cache_read_usd: None,
                    cache_write_usd: None,
                    reasoning_usd: None,
                }))
            },
        )
        .expect("reproject costs");
    assert_eq!(report.events_scanned, 1);
    assert_eq!(report.completion_events_scanned, 1);
    assert_eq!(report.completion_costs_changed, 1);

    let after = store
        .load_cost_summary(session.session_id)
        .expect("projection lookup")
        .expect("projection exists");
    assert_eq!(after.prompt_tokens, 100);
    assert_eq!(after.completion_tokens, 50);
    assert_eq!(after.total_tokens, 150);
    assert_eq!(after.total_cost_usd, 0.35);
    assert_eq!(after.unpriced_completion_count, 0);

    let stored_events = store
        .load_all_events(session.session_id)
        .expect("load stored events");
    assert_eq!(stored_events.len(), 1);
    assert_eq!(stored_events[0].seq_id, Some(1));
    assert!(
        matches!(
            &stored_events[0].payload,
            EventPayload::CompletionFinished { cost: None, .. }
        ),
        "reprojection must not mutate the canonical event log"
    );
}

#[test]
fn budget_events_update_budget_projection() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let budget = BudgetConfig {
        max_wall_clock_seconds: Some(300),
        max_tokens: Some(25_000),
        max_turns: Some(8),
        max_cost_usd: Some(5.0),
    };
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Session,
            EventPayload::BudgetConfigured {
                budget: budget.clone(),
            },
        ))
        .expect("append budget configured");
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Session,
            EventPayload::BudgetCheckpoint {
                tokens_used: 7_500,
                turns_used: 2,
                elapsed_seconds: 45,
                cost_used_usd: Some(1.25),
            },
        ))
        .expect("append budget checkpoint");

    let projection = store
        .load_session_budget(session.session_id)
        .expect("projection lookup")
        .expect("projection exists");
    assert_eq!(projection.budget, budget);
    assert_eq!(projection.tokens_used, 7_500);
    assert_eq!(projection.turns_used, 2);
    assert_eq!(projection.elapsed_seconds, 45);
    assert_eq!(projection.cost_used_usd, Some(1.25));

    let reconfigured_budget = BudgetConfig {
        max_turns: Some(12),
        ..budget
    };
    let mut reconfigured = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Session,
        EventPayload::BudgetConfigured {
            budget: reconfigured_budget.clone(),
        },
    );
    reconfigured.occurred_at = projection.updated_at + time::Duration::seconds(1);
    store
        .append_event(&reconfigured)
        .expect("append reconfigured budget");

    let reprojected = store
        .load_session_budget(session.session_id)
        .expect("reconfigured projection lookup")
        .expect("reconfigured projection exists");
    assert_eq!(reprojected.budget, reconfigured_budget);
    assert_eq!(reprojected.tokens_used, projection.tokens_used);
    assert_eq!(reprojected.turns_used, projection.turns_used);
    assert_eq!(reprojected.elapsed_seconds, projection.elapsed_seconds);
    assert_eq!(reprojected.cost_used_usd, projection.cost_used_usd);
    assert_eq!(reprojected.updated_at, reconfigured.occurred_at);
}

#[test]
fn fresh_cost_budget_starts_at_known_zero_without_cancelling() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    let configuration = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Session,
        EventPayload::BudgetConfigured {
            budget: BudgetConfig {
                max_cost_usd: Some(5.0),
                ..BudgetConfig::default()
            },
        },
    );
    let cancellation = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::SessionCancelled {
            reason: "budget_exhausted".to_owned(),
        },
    );

    let committed = store
        .commit_budget_configuration(&configuration, &cancellation)
        .expect("configure cost budget");
    assert!(!committed.exhausted);
    assert!(committed.cancellation_seq_id.is_none());
    let projection = store
        .load_session_budget(session.session_id)
        .expect("budget lookup")
        .expect("budget projection");
    assert_eq!(projection.cost_used_usd, Some(0.0));
}

#[test]
fn migration_normalizes_only_fresh_legacy_cost_budgets() {
    let file = NamedTempFile::new().expect("tempfile");
    let (fresh_session, fresh_branch) = sample_session();
    let (used_session, used_branch) = sample_session();
    {
        let mut store = SqliteSessionStore::open(file.path()).expect("open store");
        for (session, branch) in [
            (&fresh_session, &fresh_branch),
            (&used_session, &used_branch),
        ] {
            store
                .create_session(session, branch)
                .expect("create session");
            store
                .append_event(&EventEnvelope::new(
                    session.session_id,
                    branch.branch_id,
                    SpanKind::Session,
                    EventPayload::BudgetConfigured {
                        budget: BudgetConfig {
                            max_cost_usd: Some(5.0),
                            ..BudgetConfig::default()
                        },
                    },
                ))
                .expect("configure cost budget");
        }
        store
            .connection
            .execute(
                "UPDATE session_budget_projection SET cost_used_usd = NULL WHERE session_id = ?1",
                [fresh_session.session_id.to_string()],
            )
            .expect("restore fresh legacy cost state");
        store
            .connection
            .execute(
                "UPDATE session_budget_projection
                 SET cost_used_usd = NULL, turns_used = 1
                 WHERE session_id = ?1",
                [used_session.session_id.to_string()],
            )
            .expect("restore used legacy cost state");
    }

    let store = SqliteSessionStore::open(file.path()).expect("reopen migrated store");
    let fresh = store
        .load_session_budget(fresh_session.session_id)
        .expect("fresh budget lookup")
        .expect("fresh budget projection");
    assert_eq!(fresh.cost_used_usd, Some(0.0));
    assert!(!fresh.budget.is_exhausted(
        fresh.tokens_used,
        fresh.turns_used,
        fresh.elapsed_seconds,
        fresh.cost_used_usd,
    ));

    let used = store
        .load_session_budget(used_session.session_id)
        .expect("used budget lookup")
        .expect("used budget projection");
    assert_eq!(used.cost_used_usd, None);
    assert!(used.budget.is_exhausted(
        used.tokens_used,
        used.turns_used,
        used.elapsed_seconds,
        used.cost_used_usd,
    ));
}

#[test]
fn stale_budget_checkpoint_cannot_regress_durable_counters() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Session,
            EventPayload::BudgetConfigured {
                budget: BudgetConfig::default(),
            },
        ))
        .expect("configure budget");
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::BudgetCheckpoint {
                tokens_used: 20,
                turns_used: 2,
                elapsed_seconds: 10,
                cost_used_usd: Some(1.0),
            },
        ))
        .expect("current checkpoint");
    let event_count = store
        .load_all_events(session.session_id)
        .expect("events")
        .len();

    let error = store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::BudgetCheckpoint {
                tokens_used: 10,
                turns_used: 1,
                elapsed_seconds: 5,
                cost_used_usd: Some(0.5),
            },
        ))
        .expect_err("stale checkpoint must fail");
    assert!(matches!(error, BelltowerError::InvalidState(_)));
    assert_eq!(
        store
            .load_all_events(session.session_id)
            .expect("events")
            .len(),
        event_count
    );
    let projection = store
        .load_session_budget(session.session_id)
        .expect("budget lookup")
        .expect("budget projection");
    assert_eq!(projection.tokens_used, 20);
    assert_eq!(projection.turns_used, 2);
    assert_eq!(projection.elapsed_seconds, 10);
    assert_eq!(projection.cost_used_usd, Some(1.0));
}

#[test]
fn budget_checkpoint_uses_current_limits_and_cancels_atomically() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Session,
            EventPayload::BudgetConfigured {
                budget: BudgetConfig {
                    max_tokens: Some(100),
                    ..BudgetConfig::default()
                },
            },
        ))
        .expect("initial budget");
    let turn_id = TurnId::new();
    let checkpoint = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::BudgetCheckpoint {
            tokens_used: 10,
            turns_used: 1,
            elapsed_seconds: 1,
            cost_used_usd: None,
        },
    )
    .with_turn_id(turn_id);

    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Session,
            EventPayload::BudgetConfigured {
                budget: BudgetConfig {
                    max_tokens: Some(5),
                    ..BudgetConfig::default()
                },
            },
        ))
        .expect("lower budget before checkpoint commit");
    store
        .append_event(&turn_started_event(&session, &branch, turn_id))
        .expect("start owning turn");
    let cancellation = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::SessionCancelled {
            reason: "budget_exhausted".to_owned(),
        },
    );
    let committed = store
        .commit_budget_checkpoint(&checkpoint, &cancellation)
        .expect("commit checkpoint");

    assert!(committed.exhausted);
    assert!(committed.cancellation_seq_id.is_some());
    let projection = store
        .load_session_budget(session.session_id)
        .expect("load budget")
        .expect("budget exists");
    assert_eq!(projection.budget.max_tokens, Some(5));
    assert_eq!(projection.tokens_used, 10);
    assert!(
        store
            .load_session_control(session.session_id)
            .expect("load control")
            .is_some_and(|control| control.cancel_requested)
    );
}

#[test]
fn budget_terminal_transition_commits_checkpoint_cancel_and_finish_in_order() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Session,
            EventPayload::BudgetConfigured {
                budget: BudgetConfig {
                    max_turns: Some(1),
                    ..BudgetConfig::default()
                },
            },
        ))
        .expect("configure budget");
    let turn_id = TurnId::new();
    store
        .append_event(&turn_started_event(&session, &branch, turn_id))
        .expect("start turn");
    let checkpoint = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::BudgetCheckpoint {
            tokens_used: 20,
            turns_used: 1,
            elapsed_seconds: 2,
            cost_used_usd: Some(0.0),
        },
    )
    .with_turn_id(turn_id);
    let cancellation = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::SessionCancelled {
            reason: "budget_exhausted".to_owned(),
        },
    );
    let finish = turn_finished_event(&session, &branch, turn_id);

    let committed = store
        .commit_budget_terminal_transition(&checkpoint, &cancellation, &[finish])
        .expect("commit terminal transition");

    let cancellation_seq_id = committed
        .cancellation_seq_id
        .expect("budget cancellation must be committed");
    assert!(committed.checkpoint_seq_id < cancellation_seq_id);
    assert_eq!(committed.terminal_seq_ids.len(), 1);
    assert!(cancellation_seq_id < committed.terminal_seq_ids[0]);
    assert_eq!(
        store
            .load_session_inspection_metrics(session.session_id)
            .expect("metrics")
            .active_turn_count,
        0
    );
    let events = store
        .load_all_events(session.session_id)
        .expect("load events");
    let terminal_kinds = events
        .iter()
        .rev()
        .take(3)
        .map(|event| event.kind().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        terminal_kinds,
        vec![
            "turn.finished".to_owned(),
            "session.cancelled".to_owned(),
            "budget.checkpoint".to_owned(),
        ]
    );

    let event_count = events.len();
    let duplicate_finish = turn_finished_event(&session, &branch, turn_id);
    let error = store
        .commit_budget_terminal_transition(&checkpoint, &cancellation, &[duplicate_finish])
        .expect_err("finished turn cannot commit a second terminal transition");
    assert!(error.to_string().contains("does not own the active turn"));
    assert_eq!(
        store
            .load_all_events(session.session_id)
            .expect("load events after rejected duplicate")
            .len(),
        event_count
    );

    let stale_checkpoint_error = store
        .commit_budget_checkpoint(&checkpoint, &cancellation)
        .expect_err("finished turn cannot commit a later budget checkpoint");
    assert!(
        stale_checkpoint_error
            .to_string()
            .contains("does not own the active turn")
    );
    assert_eq!(
        store
            .load_all_events(session.session_id)
            .expect("load events after rejected checkpoint")
            .len(),
        event_count
    );
}

#[test]
fn unbudgeted_terminal_transition_requires_the_exact_active_turn() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    let turn_id = TurnId::new();
    store
        .append_event(&turn_started_event(&session, &branch, turn_id))
        .expect("start turn");
    let active_event = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Tool,
        EventPayload::ToolOperationRecorded {
            call_id: ToolCallId::new("call-active-owner"),
            tool_name: "read".to_owned(),
            operation: ToolOperationContext::default(),
        },
    )
    .with_turn_id(turn_id);
    store
        .commit_active_turn_events(
            session.session_id,
            branch.branch_id,
            turn_id,
            std::slice::from_ref(&active_event),
        )
        .expect("active owner can append turn evidence");
    let finish = turn_finished_event(&session, &branch, turn_id);

    let seq_ids = store
        .commit_turn_terminal_transition(session.session_id, branch.branch_id, turn_id, &[finish])
        .expect("commit terminal transition");
    assert_eq!(seq_ids.len(), 1);

    let duplicate_finish = turn_finished_event(&session, &branch, turn_id);
    let error = store
        .commit_turn_terminal_transition(
            session.session_id,
            branch.branch_id,
            turn_id,
            &[duplicate_finish],
        )
        .expect_err("finished turn cannot commit a second terminal transition");
    assert!(error.to_string().contains("does not own the active turn"));

    let event_count = store
        .load_all_events(session.session_id)
        .expect("load events before stale active append")
        .len();
    let stale_event = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Tool,
        EventPayload::ToolOperationRecorded {
            call_id: ToolCallId::new("call-stale-owner"),
            tool_name: "shell".to_owned(),
            operation: ToolOperationContext::default(),
        },
    )
    .with_turn_id(turn_id);
    let error = store
        .commit_active_turn_events(
            session.session_id,
            branch.branch_id,
            turn_id,
            &[stale_event],
        )
        .expect_err("finished turn cannot append more active evidence");
    assert!(error.to_string().contains("does not own the active turn"));
    assert_eq!(
        store
            .load_all_events(session.session_id)
            .expect("load events after stale active append")
            .len(),
        event_count
    );
}

#[test]
fn budget_configuration_is_an_idle_boundary_and_cancels_if_already_exhausted() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    let turn_id = TurnId::new();
    store
        .append_event(&turn_started_event(&session, &branch, turn_id))
        .expect("start turn");
    let configuration = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::BudgetConfigured {
            budget: BudgetConfig {
                max_turns: Some(0),
                ..BudgetConfig::default()
            },
        },
    );
    let cancellation = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::SessionCancelled {
            reason: "budget_exhausted".to_owned(),
        },
    );

    let error = store
        .commit_budget_configuration(&configuration, &cancellation)
        .expect_err("active budget change must fail");
    assert!(matches!(error, BelltowerError::InvalidState(_)));
    assert!(
        store
            .load_session_budget(session.session_id)
            .expect("budget lookup")
            .is_none()
    );

    store
        .append_event(&turn_finished_event(&session, &branch, turn_id))
        .expect("finish turn");
    let committed = store
        .commit_budget_configuration(&configuration, &cancellation)
        .expect("idle budget configuration");
    assert!(committed.exhausted);
    assert!(committed.cancellation_seq_id.is_some());
    assert!(
        store
            .load_session_control(session.session_id)
            .expect("control lookup")
            .is_some_and(|control| control.cancel_requested)
    );
}

#[test]
fn exhausted_budget_configuration_records_its_reason_after_operator_cancel() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::SessionCancelled {
                reason: "operator_request".to_owned(),
            },
        ))
        .expect("record operator cancellation");
    let configuration = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::BudgetConfigured {
            budget: BudgetConfig {
                max_turns: Some(0),
                ..BudgetConfig::default()
            },
        },
    );
    let budget_cancellation = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::SessionCancelled {
            reason: "budget_exhausted".to_owned(),
        },
    );

    let committed = store
        .commit_budget_configuration(&configuration, &budget_cancellation)
        .expect("configure exhausted budget");
    assert!(committed.exhausted);
    assert!(committed.cancellation_seq_id.is_some());
    let cancellation_reasons = store
        .load_all_events(session.session_id)
        .expect("load events")
        .into_iter()
        .filter_map(|event| match event.payload {
            EventPayload::SessionCancelled { reason } => Some(reason),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        cancellation_reasons,
        vec!["operator_request".to_owned(), "budget_exhausted".to_owned()]
    );
}

#[test]
fn budget_reprojection_restores_projection_without_mutating_events() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let budget = BudgetConfig {
        max_wall_clock_seconds: Some(120),
        max_tokens: Some(10_000),
        max_turns: Some(4),
        max_cost_usd: Some(2.0),
    };
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Session,
            EventPayload::BudgetConfigured {
                budget: budget.clone(),
            },
        ))
        .expect("append budget configured");
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Session,
            EventPayload::BudgetCheckpoint {
                tokens_used: 4_000,
                turns_used: 1,
                elapsed_seconds: 30,
                cost_used_usd: Some(0.75),
            },
        ))
        .expect("append budget checkpoint");

    store
        .connection
        .execute(
            "DELETE FROM session_budget_projection WHERE session_id = ?1",
            rusqlite::params![session.session_id.to_string()],
        )
        .expect("delete budget projection");
    assert!(
        store
            .load_session_budget(session.session_id)
            .expect("load deleted projection")
            .is_none()
    );

    store
        .reproject_session_budget(session.session_id)
        .expect("reproject budget");

    let projection = store
        .load_session_budget(session.session_id)
        .expect("projection lookup")
        .expect("projection exists");
    assert_eq!(projection.budget, budget);
    assert_eq!(projection.tokens_used, 4_000);
    assert_eq!(projection.turns_used, 1);
    assert_eq!(projection.elapsed_seconds, 30);
    assert_eq!(projection.cost_used_usd, Some(0.75));

    let stored_events = store
        .load_all_events(session.session_id)
        .expect("load stored events");
    assert_eq!(stored_events.len(), 2);
    assert!(matches!(
        &stored_events[0].payload,
        EventPayload::BudgetConfigured { .. }
    ));
    assert!(matches!(
        &stored_events[1].payload,
        EventPayload::BudgetCheckpoint { .. }
    ));
}

#[test]
fn raw_chunk_round_trip_works() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let turn_id = TurnId::new();
    store
        .append_event(&turn_started_event(&session, &branch, turn_id))
        .expect("start raw-chunk turn");
    let chunk_id = store
        .append_raw_chunk(
            session.session_id,
            branch.branch_id,
            Some(turn_id),
            None,
            None,
            "openai-compatible",
            "completion",
            b"{\"hello\":\"world\"}",
        )
        .expect("append raw chunk");
    store
        .append_event(&turn_finished_event(&session, &branch, turn_id))
        .expect("finish raw-chunk turn");
    assert_eq!(chunk_id, 1);

    let chunks = store
        .load_raw_chunks(session.session_id, 10)
        .expect("load raw chunks");
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].branch_id, Some(branch.branch_id));
    assert!(chunks[0].turn_id.is_some());
    assert_eq!(chunks[0].provider, "openai-compatible");
    assert_eq!(chunks[0].stream_name, "completion");
    assert_eq!(chunks[0].content, b"{\"hello\":\"world\"}");
}

#[test]
fn raw_chunk_pages_scope_to_branch_and_turn() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let turn_a = TurnId::new();
    let turn_b = TurnId::new();
    store
        .append_event(&turn_started_event(&session, &branch, turn_a))
        .expect("start turn a");

    for (content, ordinal) in [
        (b"one".as_slice(), Some(1_u32)),
        (b"two".as_slice(), Some(1_u32)),
        (b"three".as_slice(), Some(2_u32)),
    ] {
        store
            .append_raw_chunk(
                session.session_id,
                branch.branch_id,
                Some(turn_a),
                ordinal,
                None,
                "openai-compatible",
                "completion",
                content,
            )
            .expect("append turn-a raw chunk");
    }
    store
        .append_event(&turn_finished_event(&session, &branch, turn_a))
        .expect("finish turn a");
    store
        .append_event(&turn_started_event(&session, &branch, turn_b))
        .expect("start turn b");
    store
        .append_raw_chunk(
            session.session_id,
            branch.branch_id,
            Some(turn_b),
            None,
            None,
            "openai-compatible",
            "completion",
            b"other-turn",
        )
        .expect("append turn-b raw chunk");
    store
        .append_event(&turn_finished_event(&session, &branch, turn_b))
        .expect("finish turn b");

    let latest = store
        .load_turn_raw_chunks_page(session.session_id, branch.branch_id, turn_a, None, None, 2)
        .expect("latest turn-a page");
    assert!(latest.has_more_before);
    assert_eq!(latest.items.len(), 2);
    assert_eq!(latest.items[0].content, b"two");
    assert_eq!(latest.items[1].content, b"three");

    let older = store
        .load_turn_raw_chunks_page(
            session.session_id,
            branch.branch_id,
            turn_a,
            None,
            latest.oldest_chunk_id,
            10,
        )
        .expect("older turn-a page");
    assert!(!older.has_more_before);
    assert_eq!(older.items.len(), 1);
    assert_eq!(older.items[0].content, b"one");

    let second_call_only = store
        .load_turn_raw_chunks_page(
            session.session_id,
            branch.branch_id,
            turn_a,
            Some(2),
            None,
            10,
        )
        .expect("second llm call page");
    assert!(!second_call_only.has_more_before);
    assert_eq!(second_call_only.items.len(), 1);
    assert_eq!(second_call_only.items[0].content, b"three");
    assert_eq!(second_call_only.items[0].llm_call_ordinal, Some(2));
}

#[test]
fn export_legacy_bundle_contains_session_branches_messages_events_and_raw_chunks() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::User, "hello bundle"),
            },
        ))
        .expect("append event");
    let turn_id = TurnId::new();
    store
        .append_event(&turn_started_event(&session, &branch, turn_id))
        .expect("start legacy raw-chunk turn");
    store
        .append_raw_chunk(
            session.session_id,
            branch.branch_id,
            Some(turn_id),
            None,
            None,
            "openai-compatible",
            "completion",
            b"chunk-bytes",
        )
        .expect("append raw chunk");
    store
        .append_event(&turn_finished_event(&session, &branch, turn_id))
        .expect("finish legacy raw-chunk turn");

    let bundle = store
        .export_legacy_bundle(session.session_id)
        .expect("export legacy bundle");
    assert_eq!(
        bundle.schema_version,
        LEGACY_SESSION_EXPORT_BUNDLE_SCHEMA_VERSION
    );
    assert_eq!(bundle.session.session_id, session.session_id);
    assert_eq!(bundle.branches.len(), 1);
    assert_eq!(bundle.message_branch_id, Some(branch.branch_id));
    assert_eq!(bundle.messages.len(), 1);
    assert_eq!(bundle.events.len(), 3);
    assert_eq!(bundle.raw_chunks.len(), 1);
}

#[test]
fn portable_session_bundle_exports_and_validates_offline() {
    let (bundle_dir, session_id) = export_portable_bundle_fixture();

    let report = validate_session_bundle_directory(bundle_dir.path()).expect("validate bundle");
    assert_eq!(report.session_id, session_id);
    assert_eq!(report.event_count, 4);
    assert_eq!(report.raw_chunk_count, 1);
    assert_eq!(report.content_count, 1);

    let manifest_path = bundle_dir.path().join("manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(manifest_path).expect("manifest bytes"))
            .expect("manifest json");
    assert_eq!(manifest["session"]["session_id"], session_id.to_string());
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["redaction"], "none");

    let events =
        fs::read_to_string(bundle_dir.path().join(SESSION_BT_EVENTS)).expect("events jsonl");
    let event_lines = events.lines().collect::<Vec<_>>();
    let first_event: serde_json::Value =
        serde_json::from_str(event_lines.first().expect("first event")).expect("event json");
    assert!(first_event["event"].get("seq_id").is_none());
    let chunk_event: serde_json::Value = event_lines
        .iter()
        .map(|line| serde_json::from_str(line).expect("event json"))
        .find(|event: &serde_json::Value| event["event_kind"] == "completion.chunk")
        .expect("chunk event");
    let chunk_payload = &chunk_event["event"]["payload"]["CompletionChunk"];
    assert!(chunk_payload.get("raw_chunk_index").is_none());
    assert!(
        chunk_payload["raw_chunk_content_ref"]
            .as_str()
            .expect("raw content ref")
            .starts_with("sha256:")
    );
}

#[test]
fn portable_session_bundle_import_round_trips_canonical_evidence() {
    let (bundle_dir, session_id) = export_portable_bundle_fixture();
    let original_event_hashes = bundle_event_hashes(bundle_dir.path());

    let mut imported = SqliteSessionStore::open_in_memory().expect("import store");
    let report = imported
        .import_session_bundle_directory(bundle_dir.path())
        .expect("import portable bundle");
    assert_eq!(report.session_id, session_id);
    assert_eq!(report.branch_count, 1);
    assert_eq!(report.event_count, 4);
    assert_eq!(report.raw_chunk_count, 1);

    let loaded_session = imported
        .load_session(session_id)
        .expect("load imported session")
        .expect("imported session");
    assert_eq!(loaded_session.session_id, session_id);
    assert_eq!(loaded_session.project_root, "/tmp/project");
    assert_eq!(
        imported.load_branches(session_id).expect("branches").len(),
        1
    );
    assert_eq!(
        imported.load_all_events(session_id).expect("events").len(),
        4
    );
    assert_eq!(
        imported
            .load_all_raw_chunks(session_id)
            .expect("raw chunks")
            .len(),
        1
    );

    let reexported_dir = tempdir().expect("reexport dir");
    imported
        .export_session_bundle_directory(session_id, reexported_dir.path())
        .expect("reexport portable bundle");
    validate_session_bundle_directory(reexported_dir.path()).expect("validate reexported bundle");
    assert_eq!(
        bundle_event_hashes(reexported_dir.path()),
        original_event_hashes
    );
}

#[test]
fn portable_session_bundles_round_trip_related_session_mailboxes() {
    let file = NamedTempFile::new().expect("tempfile");
    let mut source = SqliteSessionStore::open(file.path()).expect("source store");
    let (parent, parent_branch) = sample_session();
    source
        .create_session(&parent, &parent_branch)
        .expect("create parent");
    let (child, child_branch) = child_session(&parent, &parent_branch);
    source
        .create_session(&child, &child_branch)
        .expect("create child");

    let message_id = RelatedSessionMessageId::new();
    let (sent, received) = related_message_events(
        &parent,
        &parent_branch,
        &child,
        &child_branch,
        message_id,
        "try the alternate model",
        RelatedSessionDeliveryMode::Wake,
    );
    source
        .commit_related_session_message(&sent, &received)
        .expect("commit related message");
    let turn_id = TurnId::new();
    let claim_events = vec![
        EventEnvelope::new(
            child.session_id,
            child_branch.branch_id,
            SpanKind::Chain,
            EventPayload::RelatedSessionMessageResolved {
                message_id,
                status: RelatedSessionMessageStatus::Claimed,
                resulting_turn_id: Some(turn_id),
                reason: Some("claimed before export".to_owned()),
            },
        ),
        turn_started_event_with_source(
            &child,
            &child_branch,
            turn_id,
            TurnStartSource::RelatedSessionMessage,
        ),
    ];
    assert!(matches!(
        source
            .claim_related_message_continuation(
                child.session_id,
                message_id,
                child.settings_revision_id,
                &claim_events,
            )
            .expect("claim related message"),
        ContinuationClaim::Claimed { .. }
    ));

    let parent_bundle = tempdir().expect("parent bundle");
    let child_bundle = tempdir().expect("child bundle");
    source
        .export_session_bundle_directory(parent.session_id, parent_bundle.path())
        .expect("export parent bundle");
    source
        .export_session_bundle_directory(child.session_id, child_bundle.path())
        .expect("export child bundle");

    for bundle in [&parent_bundle, &child_bundle] {
        let events = fs::read_to_string(bundle.path().join(SESSION_BT_EVENTS))
            .expect("related-session events");
        assert!(events.contains("session.related_message.recorded"));
    }

    let mut imported = SqliteSessionStore::open_in_memory().expect("import store");
    imported
        .import_session_bundle_directory(child_bundle.path())
        .expect("import child bundle");
    imported
        .import_session_bundle_directory(parent_bundle.path())
        .expect("import parent bundle after child bundle");

    let parent_messages = imported
        .load_related_session_messages(parent.session_id)
        .expect("parent mailbox");
    let child_messages = imported
        .load_related_session_messages(child.session_id)
        .expect("child mailbox");
    assert_eq!(parent_messages.len(), 1);
    assert_eq!(child_messages.len(), 1);
    assert_eq!(parent_messages[0].message.message_id, message_id);
    assert_eq!(child_messages[0].message.message_id, message_id);
    assert_eq!(
        parent_messages[0].direction,
        RelatedSessionMessageDirection::Sent
    );
    assert_eq!(
        child_messages[0].direction,
        RelatedSessionMessageDirection::Received
    );
    assert_eq!(
        parent_messages[0].status,
        RelatedSessionMessageStatus::Claimed
    );
    assert_eq!(
        child_messages[0].status,
        RelatedSessionMessageStatus::Claimed
    );
    assert_eq!(parent_messages[0].resulting_turn_id, Some(turn_id));
    assert_eq!(child_messages[0].resulting_turn_id, Some(turn_id));
}

#[test]
fn portable_session_bundle_import_rejects_existing_session() {
    let (bundle_dir, session_id) = export_portable_bundle_fixture();

    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    store
        .import_session_bundle_directory(bundle_dir.path())
        .expect("first import");

    let error = store
        .import_session_bundle_directory(bundle_dir.path())
        .expect_err("duplicate import rejected");
    assert!(
        error
            .to_string()
            .contains(&format!("session {session_id} already exists"))
    );
}

#[test]
fn portable_session_bundle_validation_rejects_untracked_bundle_files() {
    let (bundle_dir, _) = export_portable_bundle_fixture();
    fs::write(bundle_dir.path().join("extra.json"), "{}\n").expect("write untracked file");

    let error = validate_session_bundle_directory(bundle_dir.path()).expect_err("invalid bundle");
    let message = error.to_string();
    assert!(
        message.contains("bundle contains untracked file extra.json"),
        "{message}"
    );
}

#[test]
fn portable_session_bundle_import_rolls_back_when_event_denormalization_fails() {
    let (bundle_dir, session_id) = export_portable_bundle_fixture();
    let events_path = bundle_dir.path().join(SESSION_BT_EVENTS);
    let mut event_records = fs::read_to_string(&events_path)
        .expect("events jsonl")
        .lines()
        .map(|line| serde_json::from_str::<SessionBundleEventRecord>(line).expect("event record"))
        .collect::<Vec<_>>();
    let completion = event_records
        .iter_mut()
        .find(|record| record.event_kind == "completion.chunk")
        .expect("completion chunk event");
    let completion_event_id: bt_core::EventId = completion.event["event_id"]
        .as_str()
        .expect("completion event id")
        .parse()
        .expect("parse completion event id");
    let old_completion_event_hash = completion.event_hash.clone();
    completion.event["payload"]["CompletionChunk"]
        .as_object_mut()
        .expect("completion chunk payload")
        .remove("deltas");
    completion.event_hash =
        test_hash_bytes(&serde_json::to_vec(&completion.event).expect("event bytes"));
    let completion_event_hash = completion.event_hash.clone();
    let event_lines = event_records
        .iter()
        .map(|record| serde_json::to_string(record).expect("event record json"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&events_path, format!("{event_lines}\n")).expect("write events");
    refresh_bundle_checksum_for_path(bundle_dir.path(), SESSION_BT_EVENTS);

    let manifest_path = bundle_dir.path().join(SESSION_BT_MANIFEST);
    let mut manifest: SessionBundleManifest =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest bytes"))
            .expect("manifest json");
    let bundle_hash = manifest.bundle_hash.clone();
    for branch in &mut manifest.branches {
        if let Some(head) = &mut branch.head {
            head.bundle_hash = Some(bundle_hash.clone());
            if head.event_id == completion_event_id {
                head.event_hash = completion_event_hash.clone();
            }
        }
    }
    for range in &mut manifest.event_ranges {
        if range.first_event_hash == old_completion_event_hash {
            range.first_event_hash = completion_event_hash.clone();
        }
        if range.last_event_hash == old_completion_event_hash {
            range.last_event_hash = completion_event_hash.clone();
        }
    }
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("manifest bytes"),
    )
    .expect("write manifest");

    validate_session_bundle_directory(bundle_dir.path()).expect("validation still passes");

    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let error = store
        .import_session_bundle_directory(bundle_dir.path())
        .expect_err("import should fail during event denormalization");
    assert!(
        error.to_string().contains("missing field") || error.to_string().contains("deltas"),
        "{error}"
    );
    assert!(
        store
            .load_session(session_id)
            .expect("load rolled-back session")
            .is_none(),
        "failed import must not leave a partial session behind"
    );
}

#[test]
fn portable_session_bundle_continue_rejects_existing_parent_hash_mismatch() {
    let (bundle_dir, session_id) = export_portable_bundle_fixture();
    let manifest_path = bundle_dir.path().join(SESSION_BT_MANIFEST);
    let manifest: SessionBundleManifest =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest bytes"))
            .expect("manifest json");
    let first_record = fs::read_to_string(bundle_dir.path().join(SESSION_BT_EVENTS))
        .expect("events jsonl")
        .lines()
        .next()
        .map(|line| serde_json::from_str::<SessionBundleEventRecord>(line).expect("event record"))
        .expect("first event");
    let branch_manifest = manifest.branches.first().expect("branch manifest");
    let branch = BranchRecord {
        branch_id: branch_manifest.branch_id,
        session_id,
        parent_branch_id: branch_manifest.parent_branch_id,
        parent_event_id: branch_manifest.parent_event_id,
        head_event_id: None,
        summary: branch_manifest.summary.clone(),
        created_at: manifest.session.created_at,
        is_default: true,
    };
    let mut local_event: EventEnvelope =
        serde_json::from_value(first_record.event.clone()).expect("parse event");
    local_event.payload = EventPayload::MessageAppended {
        message: Message::text(Role::Assistant, "same event id, different payload"),
    };

    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    store
        .create_session(&manifest.session, &branch)
        .expect("create existing session");
    store
        .append_event(&local_event)
        .expect("append mismatched parent event");

    let error = continue_session_bundle_directory(
        &mut store,
        bundle_dir.path(),
        Some(&first_record.event_hash),
    )
    .expect_err("hash mismatch should reject continuation");
    let message = error.to_string();
    assert!(
        message.contains("does not match bundle parent hash"),
        "{message}"
    );
}

#[test]
fn portable_session_bundle_exports_explicit_artifact_refs() {
    let project_root = tempdir().expect("project root");
    let artifact_path = project_root.path().join("paper.md");
    fs::write(&artifact_path, "# Result\n\nEvidence-backed draft.\n").expect("write artifact");

    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (mut session, branch) = sample_session();
    session.project_root = project_root.path().display().to_string().into();
    store
        .create_session(&session, &branch)
        .expect("create session");
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::Assistant, "drafted artifact"),
            },
        ))
        .expect("append event");

    let bundle_dir = tempdir().expect("bundle dir");
    let options = SessionBundleExportOptions {
        artifact_mode: SessionBundleArtifactMode::TracePlusArtifacts,
        artifacts: vec![SessionBundleArtifactInput::file("paper.md", "paper")],
    };
    let manifest = store
        .export_session_bundle_directory_with_options(
            session.session_id,
            bundle_dir.path(),
            &options,
        )
        .expect("export bundle with artifact");
    assert_eq!(
        manifest.artifact_mode,
        SessionBundleArtifactMode::TracePlusArtifacts
    );
    assert_eq!(manifest.artifacts.len(), 1);
    let artifact = &manifest.artifacts[0];
    assert_eq!(artifact.kind, "paper");
    let artifact_content_path = artifact
        .content
        .path
        .as_deref()
        .expect("artifact content path");
    assert!(artifact_content_path.starts_with(&format!("{SESSION_BT_ARTIFACTS_DIR}/")));
    assert_eq!(
        fs::read(bundle_dir.path().join(artifact_content_path)).expect("artifact content"),
        b"# Result\n\nEvidence-backed draft.\n"
    );

    let report = validate_session_bundle_directory(bundle_dir.path()).expect("validate bundle");
    assert_eq!(report.event_count, 1);
    assert_eq!(report.raw_chunk_count, 0);
    assert_eq!(report.artifact_count, 1);
}

#[test]
fn portable_session_bundle_export_rejects_root_escaping_artifact() {
    let project_root = tempdir().expect("project root");
    let outside_root = tempdir().expect("outside root");
    let outside_artifact_path = outside_root.path().join("secret.txt");
    fs::write(&outside_artifact_path, "not portable").expect("write outside artifact");

    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (mut session, branch) = sample_session();
    session.project_root = project_root.path().display().to_string().into();
    store
        .create_session(&session, &branch)
        .expect("create session");
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::Assistant, "created artifact"),
            },
        ))
        .expect("append event");

    let bundle_dir = tempdir().expect("bundle dir");
    let options = SessionBundleExportOptions {
        artifact_mode: SessionBundleArtifactMode::TracePlusArtifacts,
        artifacts: vec![SessionBundleArtifactInput::file(
            outside_artifact_path,
            "artifact",
        )],
    };
    let error = store
        .export_session_bundle_directory_with_options(
            session.session_id,
            bundle_dir.path(),
            &options,
        )
        .expect_err("root-escaping artifact rejected");
    assert!(error.to_string().contains("escapes project root"));
}

#[test]
fn portable_session_bundle_validation_rejects_missing_artifact_content() {
    let project_root = tempdir().expect("project root");
    fs::write(project_root.path().join("figure.svg"), "<svg></svg>").expect("write artifact");

    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (mut session, branch) = sample_session();
    session.project_root = project_root.path().display().to_string().into();
    store
        .create_session(&session, &branch)
        .expect("create session");
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::Assistant, "created figure"),
            },
        ))
        .expect("append event");

    let bundle_dir = tempdir().expect("bundle dir");
    let options = SessionBundleExportOptions {
        artifact_mode: SessionBundleArtifactMode::TracePlusArtifacts,
        artifacts: vec![SessionBundleArtifactInput::file("figure.svg", "figure")],
    };
    let manifest = store
        .export_session_bundle_directory_with_options(
            session.session_id,
            bundle_dir.path(),
            &options,
        )
        .expect("export bundle with artifact");
    let artifact_path = manifest.artifacts[0]
        .content
        .path
        .as_deref()
        .expect("artifact path");
    fs::remove_file(bundle_dir.path().join(artifact_path)).expect("remove artifact");

    let error = validate_session_bundle_directory(bundle_dir.path()).expect_err("invalid bundle");
    let message = error.to_string();
    assert!(
        message.contains("checksum path") && message.contains("does not exist in bundle"),
        "{message}"
    );
}

#[test]
fn portable_session_bundle_diff_reports_artifact_inventory_drift() {
    let project_root = tempdir().expect("project root");
    fs::write(project_root.path().join("report.md"), "report").expect("write artifact");

    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (mut session, branch) = sample_session();
    session.project_root = project_root.path().display().to_string().into();
    store
        .create_session(&session, &branch)
        .expect("create session");
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::Assistant, "created report"),
            },
        ))
        .expect("append event");

    let trace_only_dir = tempdir().expect("trace-only dir");
    store
        .export_session_bundle_directory(session.session_id, trace_only_dir.path())
        .expect("export trace-only bundle");

    let artifact_dir = tempdir().expect("artifact dir");
    let options = SessionBundleExportOptions {
        artifact_mode: SessionBundleArtifactMode::TracePlusArtifacts,
        artifacts: vec![SessionBundleArtifactInput::file("report.md", "report")],
    };
    store
        .export_session_bundle_directory_with_options(
            session.session_id,
            artifact_dir.path(),
            &options,
        )
        .expect("export artifact bundle");

    let report = diff_session_bundle_directories(trace_only_dir.path(), artifact_dir.path())
        .expect("diff bundles");
    assert_eq!(
        report.relationship,
        SessionBundleDiffRelationship::StructuralDifference
    );
    assert!(report.left_only_artifact_ids.is_empty());
    assert_eq!(report.right_only_artifact_ids.len(), 1);
}

#[test]
fn portable_session_bundle_diff_reports_equivalent_reexport() {
    let (bundle_dir, session_id) = export_portable_bundle_fixture();
    let reexported_dir =
        import_bundle_append_message_and_export(bundle_dir.path(), session_id, None);

    let report =
        diff_session_bundle_directories(bundle_dir.path(), reexported_dir.path()).expect("diff");
    assert_eq!(
        report.relationship,
        SessionBundleDiffRelationship::Equivalent
    );
    assert_eq!(report.common_event_count, 4);
    assert_eq!(report.left_only_event_count, 0);
    assert_eq!(report.right_only_event_count, 0);
    assert!(report.left_only_content_hashes.is_empty());
    assert!(report.right_only_content_hashes.is_empty());
}

#[test]
fn portable_session_bundle_diff_reports_same_lineage_update() {
    let (bundle_dir, session_id) = export_portable_bundle_fixture();
    let updated_dir =
        import_bundle_append_message_and_export(bundle_dir.path(), session_id, Some("new work"));

    let report =
        diff_session_bundle_directories(bundle_dir.path(), updated_dir.path()).expect("diff");
    assert_eq!(
        report.relationship,
        SessionBundleDiffRelationship::SameLineageUpdate
    );
    assert_eq!(report.common_event_count, 4);
    assert_eq!(report.left_only_event_count, 0);
    assert_eq!(report.right_only_event_count, 1);
}

#[test]
fn portable_session_bundle_diff_reports_fork_divergence() {
    let (bundle_dir, session_id) = export_portable_bundle_fixture();
    let left_dir =
        import_bundle_append_message_and_export(bundle_dir.path(), session_id, Some("left fork"));
    let right_dir =
        import_bundle_append_message_and_export(bundle_dir.path(), session_id, Some("right fork"));

    let report = diff_session_bundle_directories(left_dir.path(), right_dir.path()).expect("diff");
    assert_eq!(
        report.relationship,
        SessionBundleDiffRelationship::ForkDivergence
    );
    assert_eq!(report.common_event_count, 4);
    assert_eq!(report.left_only_event_count, 1);
    assert_eq!(report.right_only_event_count, 1);
}

#[test]
fn portable_session_bundle_continue_from_default_head_imports_and_activates_branch() {
    let (bundle_dir, session_id) = export_portable_bundle_fixture();

    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let report =
        continue_session_bundle_directory(&mut store, bundle_dir.path(), None).expect("continue");
    assert_eq!(report.session_id, session_id);
    assert!(report.imported_session);

    let branches = store.load_branches(session_id).expect("branches");
    assert_eq!(branches.len(), 2);
    let continuation = branches
        .iter()
        .find(|branch| branch.branch_id == report.branch_id)
        .expect("continuation branch");
    assert!(continuation.is_default);
    assert_eq!(continuation.parent_branch_id, Some(report.parent_branch_id));
    assert_eq!(continuation.parent_event_id, Some(report.parent_event_id));

    let events = store.load_all_events(session_id).expect("events");
    let branch_created = events
        .iter()
        .find(|event| event.branch_id == report.branch_id)
        .expect("branch created event");
    let parent_ref: SessionNodeRef = serde_json::from_value(
        branch_created
            .attributes
            .get(SESSION_BT_PARENT_NODE_REF_ATTR)
            .expect("parent node ref")
            .clone(),
    )
    .expect("parent ref value");
    assert_eq!(parent_ref.event_hash, report.parent_event_hash);
    assert_eq!(parent_ref.event_id, report.parent_event_id);
}

#[test]
fn portable_session_bundle_continue_from_mid_node_bounds_context() {
    let (bundle_dir, session_id) = export_two_message_portable_bundle_fixture();
    let first_event_hash = bundle_event_hashes(bundle_dir.path())
        .first()
        .cloned()
        .expect("first event hash");

    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let report =
        continue_session_bundle_directory(&mut store, bundle_dir.path(), Some(&first_event_hash))
            .expect("continue from first event");

    let messages = store
        .load_messages(session_id, Some(report.branch_id))
        .expect("continuation messages");
    assert_eq!(messages.len(), 2);
    assert!(matches!(messages[0].role, Role::System));
    assert!(
        matches!(messages[1].parts.as_slice(), [bt_core::MessagePart::Text { text }] if text == "first")
    );
    assert!(
        !messages.iter().any(|message| {
            matches!(message.parts.as_slice(), [bt_core::MessagePart::Text { text }] if text == "second")
        }),
        "continuation context must not include events after the selected node"
    );
}

#[test]
fn portable_session_bundle_validation_rejects_tampered_events() {
    let (bundle_dir, _) = export_portable_bundle_fixture();
    let events_path = bundle_dir.path().join(SESSION_BT_EVENTS);
    let mut events = fs::read_to_string(&events_path).expect("events jsonl");
    events.push_str("\n");
    fs::write(&events_path, events).expect("tamper events");

    let error = validate_session_bundle_directory(bundle_dir.path()).expect_err("invalid bundle");
    assert!(error.to_string().contains("checksum mismatch"));
}

#[test]
fn portable_session_bundle_validation_rejects_missing_raw_chunk_content() {
    let (bundle_dir, _) = export_portable_bundle_fixture();
    let raw_refs =
        fs::read_to_string(bundle_dir.path().join("raw_chunk_refs.jsonl")).expect("raw refs jsonl");
    let raw_ref: serde_json::Value =
        serde_json::from_str(raw_refs.lines().next().expect("raw ref")).expect("raw ref json");
    let raw_path = raw_ref["content"]["path"]
        .as_str()
        .expect("raw content path");
    fs::remove_file(bundle_dir.path().join(raw_path)).expect("remove raw chunk");

    let error = validate_session_bundle_directory(bundle_dir.path()).expect_err("invalid bundle");
    let message = error.to_string();
    assert!(
        message.contains("checksum path") && message.contains("does not exist in bundle"),
        "{message}"
    );
}

#[test]
fn portable_session_bundle_validation_requires_manifest_raw_chunk_inventory() {
    let (bundle_dir, _) = export_portable_bundle_fixture();
    let manifest_path = bundle_dir.path().join(SESSION_BT_MANIFEST);
    let mut manifest: SessionBundleManifest =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest bytes"))
            .expect("manifest json");
    manifest.contents.clear();
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("manifest bytes"),
    )
    .expect("write manifest");

    let error = validate_session_bundle_directory(bundle_dir.path()).expect_err("invalid bundle");
    let message = error.to_string();
    assert!(
        message.contains("checksum manifest contains unreferenced path")
            || message.contains("missing from manifest contents"),
        "{message}"
    );
}

#[test]
fn portable_session_bundle_validation_requires_event_raw_refs_to_resolve_to_raw_refs() {
    let (bundle_dir, _) = export_portable_bundle_fixture();
    let raw_refs_path = bundle_dir.path().join(SESSION_BT_RAW_CHUNK_REFS);
    fs::write(&raw_refs_path, "").expect("clear raw refs");

    let checksums_path = bundle_dir.path().join(SESSION_BT_CHECKSUMS);
    let mut checksums: crate::SessionBundleChecksums =
        serde_json::from_slice(&fs::read(&checksums_path).expect("checksums bytes"))
            .expect("checksums json");
    let raw_refs_entry = checksums
        .files
        .iter_mut()
        .find(|entry| entry.path == SESSION_BT_RAW_CHUNK_REFS)
        .expect("raw refs checksum entry");
    raw_refs_entry.hash = test_hash_bytes(b"");
    raw_refs_entry.size_bytes = 0;
    let checksums_bytes = serde_json::to_vec_pretty(&checksums).expect("checksums bytes");
    fs::write(&checksums_path, &checksums_bytes).expect("write checksums");

    let manifest_path = bundle_dir.path().join(SESSION_BT_MANIFEST);
    let mut manifest: SessionBundleManifest =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest bytes"))
            .expect("manifest json");
    for content in &mut manifest.contents {
        content.path = None;
    }
    manifest.bundle_hash = test_hash_bytes(&checksums_bytes);
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("manifest bytes"),
    )
    .expect("write manifest");

    let error = validate_session_bundle_directory(bundle_dir.path()).expect_err("invalid bundle");
    let message = error.to_string();
    assert!(
        message.contains("checksum manifest contains unreferenced path")
            || message.contains("missing from raw_chunk_refs"),
        "{message}"
    );
}

#[test]
fn portable_session_bundle_validation_rejects_dangling_raw_chunk_event_ref() {
    let (bundle_dir, _) = export_portable_bundle_fixture();
    let raw_refs_path = bundle_dir.path().join(SESSION_BT_RAW_CHUNK_REFS);
    let mut raw_refs = fs::read_to_string(&raw_refs_path)
        .expect("raw refs")
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("raw ref json"))
        .collect::<Vec<_>>();
    raw_refs[0]["event_id"] = serde_json::Value::String(bt_core::EventId::new().to_string());
    let raw_refs_jsonl = raw_refs
        .into_iter()
        .map(|value| serde_json::to_string(&value).expect("raw ref line"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&raw_refs_path, format!("{raw_refs_jsonl}\n")).expect("write raw refs");
    refresh_bundle_checksum_for_path(bundle_dir.path(), SESSION_BT_RAW_CHUNK_REFS);

    let error = validate_session_bundle_directory(bundle_dir.path()).expect_err("invalid bundle");
    assert!(error.to_string().contains("references missing event"));
}

#[test]
fn portable_session_bundle_validation_rejects_branch_head_tuple_mismatch() {
    let (bundle_dir, _) = export_portable_bundle_fixture();
    let manifest_path = bundle_dir.path().join(SESSION_BT_MANIFEST);
    let mut manifest: SessionBundleManifest =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest bytes"))
            .expect("manifest json");
    let head = manifest
        .branches
        .iter_mut()
        .find_map(|branch| branch.head.as_mut())
        .expect("branch head");
    head.bundle_event_ordinal += 1;
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("manifest bytes"),
    )
    .expect("write manifest");

    let error = validate_session_bundle_directory(bundle_dir.path()).expect_err("invalid bundle");
    assert!(error.to_string().contains("head does not match"));
}

#[test]
fn portable_session_bundle_validation_requires_event_scoped_raw_chunk_refs() {
    let (bundle_dir, _) = export_duplicate_raw_content_portable_bundle_fixture();
    let raw_refs_path = bundle_dir.path().join(SESSION_BT_RAW_CHUNK_REFS);
    let mut raw_refs = fs::read_to_string(&raw_refs_path)
        .expect("raw refs")
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("raw ref json"))
        .collect::<Vec<_>>();
    assert_eq!(raw_refs.len(), 2);
    raw_refs.remove(0);
    let raw_refs_jsonl = raw_refs
        .into_iter()
        .map(|value| serde_json::to_string(&value).expect("raw ref line"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&raw_refs_path, format!("{raw_refs_jsonl}\n")).expect("write raw refs");
    refresh_bundle_checksum_for_path(bundle_dir.path(), SESSION_BT_RAW_CHUNK_REFS);

    let error = validate_session_bundle_directory(bundle_dir.path()).expect_err("invalid bundle");
    assert!(
        error
            .to_string()
            .contains("missing from raw_chunk_refs for that event")
    );
}

#[test]
fn portable_session_bundle_import_preserves_event_scoped_duplicate_raw_content_refs() {
    let (bundle_dir, session_id) = export_duplicate_raw_content_portable_bundle_fixture();

    let mut imported = SqliteSessionStore::open_in_memory().expect("import store");
    imported
        .import_session_bundle_directory(bundle_dir.path())
        .expect("import bundle");
    let raw_chunks = imported
        .load_all_raw_chunks(session_id)
        .expect("raw chunks");
    assert_eq!(raw_chunks.len(), 2);
    let chunk_ids = imported
        .load_all_events(session_id)
        .expect("events")
        .into_iter()
        .filter_map(|event| match event.payload {
            EventPayload::CompletionChunk {
                raw_chunk_index: Some(chunk_id),
                ..
            } => Some(chunk_id),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(chunk_ids.len(), 2);
    assert_eq!(
        chunk_ids,
        raw_chunks
            .iter()
            .map(|chunk| chunk.chunk_id)
            .collect::<BTreeSet<_>>()
    );
}

#[test]
fn portable_session_bundle_import_preserves_one_raw_chunk_referenced_by_multiple_events() {
    let file = NamedTempFile::new().expect("tempfile");
    let mut store = SqliteSessionStore::open(file.path()).expect("open store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    let turn_id = TurnId::new();
    store
        .append_event(&turn_started_event(&session, &branch, turn_id))
        .expect("start portable raw-chunk turn");
    let (chunk_id, raw_event, _) = store
        .append_raw_chunk_with_event(
            session.session_id,
            branch.branch_id,
            Some(turn_id),
            Some(1),
            "openai-compatible",
            "completion",
            b"runtime-raw-bytes",
            |chunk_id| {
                Ok(EventEnvelope::new(
                    session.session_id,
                    branch.branch_id,
                    SpanKind::Llm,
                    EventPayload::RawChunkPersisted {
                        provider: "openai-compatible".to_owned(),
                        chunk_index: chunk_id,
                        stream: "completion".to_owned(),
                        llm_call_ordinal: Some(1),
                    },
                )
                .with_turn_id(turn_id))
            },
        )
        .expect("append raw persisted event");
    store
        .append_event(
            &EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Llm,
                EventPayload::CompletionChunk {
                    llm_call_ordinal: Some(1),
                    deltas: Vec::new(),
                    raw_chunk_index: Some(chunk_id),
                },
            )
            .with_turn_id(turn_id),
        )
        .expect("append completion chunk event");
    store
        .append_event(&turn_finished_event(&session, &branch, turn_id))
        .expect("finish portable raw-chunk turn");
    drop(store);

    let reopened = SqliteSessionStore::open(file.path()).expect("reopen store");
    let bundle_dir = tempdir().expect("bundle dir");
    reopened
        .export_session_bundle_directory(session.session_id, bundle_dir.path())
        .expect("export portable bundle");
    validate_session_bundle_directory(bundle_dir.path()).expect("validate bundle");

    let raw_refs = fs::read_to_string(bundle_dir.path().join(SESSION_BT_RAW_CHUNK_REFS))
        .expect("raw refs")
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("raw ref json"))
        .collect::<Vec<_>>();
    assert_eq!(raw_refs.len(), 2);
    assert!(
        raw_refs
            .iter()
            .all(|value| value.get("local_chunk_id").is_none())
    );
    let chunk_ordinal = raw_refs[0]["chunk_ordinal"]
        .as_u64()
        .expect("chunk ordinal");
    assert!(
        raw_refs
            .iter()
            .all(|value| value["chunk_ordinal"].as_u64() == Some(chunk_ordinal))
    );
    assert!(
        raw_refs
            .iter()
            .any(|value| value["event_id"] == raw_event.event_id.to_string())
    );

    let mut imported = SqliteSessionStore::open_in_memory().expect("import store");
    imported
        .import_session_bundle_directory(bundle_dir.path())
        .expect("import bundle");
    let raw_chunks = imported
        .load_all_raw_chunks(session.session_id)
        .expect("raw chunks");
    assert_eq!(raw_chunks.len(), 1);
    let imported_chunk_id = raw_chunks[0].chunk_id;
    let event_chunk_ids = imported
        .load_all_events(session.session_id)
        .expect("events")
        .into_iter()
        .filter_map(|event| match event.payload {
            EventPayload::RawChunkPersisted { chunk_index, .. } => Some(chunk_index),
            EventPayload::CompletionChunk {
                raw_chunk_index: Some(chunk_index),
                ..
            } => Some(chunk_index),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(event_chunk_ids, vec![imported_chunk_id, imported_chunk_id]);
}

#[test]
fn portable_session_bundle_splits_interleaved_branch_ranges() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, parent) = sample_session();
    store
        .create_session(&session, &parent)
        .expect("create session");

    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            parent.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::User, "parent first"),
            },
        ))
        .expect("append parent first");
    let parent = store
        .load_branch(session.session_id, parent.branch_id)
        .expect("load parent")
        .expect("parent branch");
    let child = BranchRecord {
        branch_id: bt_core::BranchId::new(),
        session_id: session.session_id,
        parent_branch_id: Some(parent.branch_id),
        parent_event_id: parent.head_event_id,
        head_event_id: None,
        summary: Some("child".to_owned()),
        created_at: OffsetDateTime::now_utc(),
        is_default: false,
    };
    store.create_branch(&child).expect("create child branch");
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            child.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::User, "child"),
            },
        ))
        .expect("append child");
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            parent.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::Assistant, "parent second"),
            },
        ))
        .expect("append parent second");

    let bundle_dir = tempdir().expect("bundle dir");
    store
        .export_session_bundle_directory(session.session_id, bundle_dir.path())
        .expect("export portable bundle");
    validate_session_bundle_directory(bundle_dir.path()).expect("validate bundle");

    let manifest_path = bundle_dir.path().join("manifest.json");
    let mut manifest: SessionBundleManifest =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest bytes"))
            .expect("manifest json");
    assert_eq!(manifest.event_ranges.len(), 3);
    assert!(
        manifest
            .event_ranges
            .iter()
            .all(|range| range.ordinal_start == range.ordinal_end)
    );

    manifest.event_ranges[0].ordinal_end = 2;
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("manifest bytes"),
    )
    .expect("write manifest");
    let error = validate_session_bundle_directory(bundle_dir.path()).expect_err("invalid bundle");
    assert!(error.to_string().contains("crosses branch boundary"));
}

#[test]
fn portable_session_bundle_exports_branch_fork_boundary() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let first = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::MessageAppended {
            message: Message::text(Role::User, "first"),
        },
    );
    let second = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::MessageAppended {
            message: Message::text(Role::Assistant, "second"),
        },
    );
    store.append_event(&first).expect("append first");
    store.append_event(&second).expect("append second");

    let child = BranchRecord {
        branch_id: bt_core::BranchId::new(),
        session_id: session.session_id,
        parent_branch_id: Some(branch.branch_id),
        parent_event_id: Some(first.event_id),
        head_event_id: None,
        summary: Some("rewind from first".to_owned()),
        created_at: OffsetDateTime::now_utc(),
        is_default: false,
    };
    store.create_branch(&child).expect("create child branch");
    let child_event = EventEnvelope::new(
        session.session_id,
        child.branch_id,
        SpanKind::Agent,
        EventPayload::MessageAppended {
            message: Message::text(Role::User, "child"),
        },
    );
    store.append_event(&child_event).expect("append child");

    let bundle_dir = tempdir().expect("bundle dir");
    store
        .export_session_bundle_directory(session.session_id, bundle_dir.path())
        .expect("export portable bundle");
    validate_session_bundle_directory(bundle_dir.path()).expect("validate bundle");

    let manifest_path = bundle_dir.path().join(SESSION_BT_MANIFEST);
    let manifest: SessionBundleManifest =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest bytes"))
            .expect("manifest json");
    let child_manifest = manifest
        .branches
        .iter()
        .find(|candidate| candidate.branch_id == child.branch_id)
        .expect("child branch manifest");
    assert_eq!(child_manifest.parent_branch_id, Some(branch.branch_id));
    assert_eq!(child_manifest.parent_event_id, Some(first.event_id));
    assert_eq!(
        child_manifest.head.as_ref().map(|head| head.event_id),
        Some(child_event.event_id)
    );
    assert!(
        manifest
            .event_ranges
            .iter()
            .any(|range| range.branch_id == child.branch_id
                && range.ordinal_start == range.ordinal_end),
        "child branch events must be a distinct portable event range"
    );
}

fn test_hash_bytes(bytes: &[u8]) -> PortableHash {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut hex, "{byte:02x}").expect("write hash hex");
    }
    PortableHash::from_sha256_hex(hex)
}

fn refresh_bundle_checksum_for_path(bundle_dir: &std::path::Path, relative_path: &str) {
    let checksums_path = bundle_dir.join(SESSION_BT_CHECKSUMS);
    let mut checksums: crate::SessionBundleChecksums =
        serde_json::from_slice(&fs::read(&checksums_path).expect("checksums bytes"))
            .expect("checksums json");
    let bytes = fs::read(bundle_dir.join(relative_path)).expect("file bytes");
    let entry = checksums
        .files
        .iter_mut()
        .find(|entry| entry.path == relative_path)
        .expect("checksum entry");
    entry.hash = test_hash_bytes(&bytes);
    entry.size_bytes = bytes.len() as u64;
    let checksums_bytes = serde_json::to_vec_pretty(&checksums).expect("checksums bytes");
    fs::write(&checksums_path, &checksums_bytes).expect("write checksums");

    let manifest_path = bundle_dir.join(SESSION_BT_MANIFEST);
    let mut manifest: SessionBundleManifest =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest bytes"))
            .expect("manifest json");
    manifest.bundle_hash = test_hash_bytes(&checksums_bytes);
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("manifest bytes"),
    )
    .expect("write manifest");
}

fn bundle_event_hashes(bundle_dir: &std::path::Path) -> Vec<PortableHash> {
    fs::read_to_string(bundle_dir.join(SESSION_BT_EVENTS))
        .expect("events jsonl")
        .lines()
        .map(|line| {
            serde_json::from_str::<SessionBundleEventRecord>(line)
                .expect("event record")
                .event_hash
        })
        .collect()
}

fn export_portable_bundle_fixture() -> (tempfile::TempDir, bt_core::SessionId) {
    let file = NamedTempFile::new().expect("tempfile");
    let mut store = SqliteSessionStore::open(file.path()).expect("open store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::User, "hello portable bundle"),
            },
        ))
        .expect("append message");
    let turn_id = TurnId::new();
    store
        .append_event(&turn_started_event(&session, &branch, turn_id))
        .expect("start portable fixture turn");
    store
        .append_raw_chunk_with_event(
            session.session_id,
            branch.branch_id,
            Some(turn_id),
            Some(1),
            "openai-compatible",
            "completion",
            b"raw-bytes",
            |chunk_id| {
                Ok(EventEnvelope::new(
                    session.session_id,
                    branch.branch_id,
                    SpanKind::Llm,
                    EventPayload::CompletionChunk {
                        llm_call_ordinal: Some(1),
                        deltas: Vec::new(),
                        raw_chunk_index: Some(chunk_id),
                    },
                )
                .with_turn_id(turn_id))
            },
        )
        .expect("append raw chunk event");
    store
        .append_event(&turn_finished_event(&session, &branch, turn_id))
        .expect("finish portable fixture turn");

    drop(store);
    let reopened = SqliteSessionStore::open(file.path()).expect("reopen store");
    let bundle_dir = tempdir().expect("bundle dir");
    reopened
        .export_session_bundle_directory(session.session_id, bundle_dir.path())
        .expect("export portable bundle");
    (bundle_dir, session.session_id)
}

fn export_duplicate_raw_content_portable_bundle_fixture() -> (tempfile::TempDir, bt_core::SessionId)
{
    let file = NamedTempFile::new().expect("tempfile");
    let mut store = SqliteSessionStore::open(file.path()).expect("open store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    for ordinal in 1..=2 {
        let turn_id = TurnId::new();
        store
            .append_event(&turn_started_event(&session, &branch, turn_id))
            .expect("start duplicate raw-chunk turn");
        store
            .append_raw_chunk_with_event(
                session.session_id,
                branch.branch_id,
                Some(turn_id),
                Some(ordinal),
                "openai-compatible",
                "completion",
                b"same-raw-bytes",
                |chunk_id| {
                    Ok(EventEnvelope::new(
                        session.session_id,
                        branch.branch_id,
                        SpanKind::Llm,
                        EventPayload::CompletionChunk {
                            llm_call_ordinal: Some(ordinal),
                            deltas: Vec::new(),
                            raw_chunk_index: Some(chunk_id),
                        },
                    )
                    .with_turn_id(turn_id))
                },
            )
            .expect("append duplicate raw chunk event");
        store
            .append_event(&turn_finished_event(&session, &branch, turn_id))
            .expect("finish duplicate raw-chunk turn");
    }
    drop(store);

    let reopened = SqliteSessionStore::open(file.path()).expect("reopen store");
    let bundle_dir = tempdir().expect("bundle dir");
    reopened
        .export_session_bundle_directory(session.session_id, bundle_dir.path())
        .expect("export portable bundle");
    (bundle_dir, session.session_id)
}

fn export_two_message_portable_bundle_fixture() -> (tempfile::TempDir, bt_core::SessionId) {
    let file = NamedTempFile::new().expect("tempfile");
    let mut store = SqliteSessionStore::open(file.path()).expect("open store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    for text in ["first", "second"] {
        store
            .append_event(&EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Agent,
                EventPayload::MessageAppended {
                    message: Message::text(Role::User, text),
                },
            ))
            .expect("append message");
    }

    drop(store);
    let reopened = SqliteSessionStore::open(file.path()).expect("reopen store");
    let bundle_dir = tempdir().expect("bundle dir");
    reopened
        .export_session_bundle_directory(session.session_id, bundle_dir.path())
        .expect("export portable bundle");
    (bundle_dir, session.session_id)
}

fn import_bundle_append_message_and_export(
    bundle_dir: &std::path::Path,
    session_id: bt_core::SessionId,
    message: Option<&str>,
) -> tempfile::TempDir {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    store
        .import_session_bundle_directory(bundle_dir)
        .expect("import bundle");
    if let Some(message) = message {
        let branch = store
            .load_branches(session_id)
            .expect("branches")
            .into_iter()
            .find(|branch| branch.is_default)
            .expect("default branch");
        store
            .append_event(&EventEnvelope::new(
                session_id,
                branch.branch_id,
                SpanKind::Agent,
                EventPayload::MessageAppended {
                    message: Message::text(Role::Assistant, message),
                },
            ))
            .expect("append message");
    }
    let output_dir = tempdir().expect("output dir");
    store
        .export_session_bundle_directory(session_id, output_dir.path())
        .expect("export bundle");
    output_dir
}

#[test]
fn branch_messages_include_parent_history_up_to_fork_point() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let first = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::MessageAppended {
            message: Message::text(Role::User, "first"),
        },
    );
    let second = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::MessageAppended {
            message: Message::text(Role::Assistant, "second"),
        },
    );
    store.append_event(&first).expect("append first");
    store.append_event(&second).expect("append second");

    let parent = store
        .load_branch(session.session_id, branch.branch_id)
        .expect("load branch")
        .expect("branch exists");
    let child = BranchRecord {
        branch_id: bt_core::BranchId::new(),
        session_id: session.session_id,
        parent_branch_id: Some(parent.branch_id),
        parent_event_id: Some(first.event_id),
        head_event_id: None,
        summary: Some("handoff".to_owned()),
        created_at: OffsetDateTime::now_utc(),
        is_default: false,
    };
    store.create_branch(&child).expect("create child branch");

    let child_event = EventEnvelope::new(
        session.session_id,
        child.branch_id,
        SpanKind::Agent,
        EventPayload::MessageAppended {
            message: Message::text(Role::User, "child"),
        },
    );
    store.append_event(&child_event).expect("append child");

    let messages = store
        .load_messages(session.session_id, Some(child.branch_id))
        .expect("load branch messages");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].role, Role::System);
    assert!(
        matches!(messages[1].parts.as_slice(), [bt_core::MessagePart::Text { text }] if text == "first")
    );
    assert!(
        matches!(messages[2].parts.as_slice(), [bt_core::MessagePart::Text { text }] if text == "child")
    );
}

#[test]
fn branch_message_and_command_pages_load_latest_window_then_backfill_older_history() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    for text in ["first", "second", "third"] {
        store
            .append_event(&EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Agent,
                EventPayload::MessageAppended {
                    message: Message::text(Role::User, text),
                },
            ))
            .expect("append message");
    }
    for input in ["/one", "/two", "/three"] {
        store
            .append_event(&EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Agent,
                EventPayload::OperatorCommandRecorded {
                    command_type: "slash_command".to_owned(),
                    raw_input: input.to_owned(),
                    output: format!("output {input}"),
                    success: true,
                },
            ))
            .expect("append operator command");
    }

    let parent = store
        .load_branch(session.session_id, branch.branch_id)
        .expect("load branch")
        .expect("branch exists");
    let child = BranchRecord {
        branch_id: bt_core::BranchId::new(),
        session_id: session.session_id,
        parent_branch_id: Some(parent.branch_id),
        parent_event_id: parent.head_event_id,
        head_event_id: None,
        summary: Some("handoff".to_owned()),
        created_at: OffsetDateTime::now_utc(),
        is_default: false,
    };
    store.create_branch(&child).expect("create child branch");
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            child.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::Assistant, "child"),
            },
        ))
        .expect("append child message");
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            child.branch_id,
            SpanKind::Agent,
            EventPayload::OperatorCommandRecorded {
                command_type: "slash_command".to_owned(),
                raw_input: "/child".to_owned(),
                output: "output /child".to_owned(),
                success: true,
            },
        ))
        .expect("append child operator command");

    let latest_messages = store
        .load_branch_messages_page(session.session_id, child.branch_id, None, 2)
        .expect("latest message page");
    assert!(latest_messages.has_more_before);
    assert_eq!(latest_messages.items.len(), 2);
    assert!(matches!(
        latest_messages.items[0].message.parts.as_slice(),
        [bt_core::MessagePart::Text { text }] if text == "third"
    ));
    assert!(matches!(
        latest_messages.items[1].message.parts.as_slice(),
        [bt_core::MessagePart::Text { text }] if text == "child"
    ));

    let older_messages = store
        .load_branch_messages_page(
            session.session_id,
            child.branch_id,
            latest_messages.oldest_seq_id,
            10,
        )
        .expect("older message page");
    assert!(!older_messages.has_more_before);
    assert_eq!(older_messages.items[0].seq_id, 0);
    assert!(matches!(
        older_messages.items[0].message.parts.as_slice(),
        [bt_core::MessagePart::Text { text }] if text.contains("handoff")
    ));
    assert!(matches!(
        older_messages.items[1].message.parts.as_slice(),
        [bt_core::MessagePart::Text { text }] if text == "first"
    ));
    assert!(matches!(
        older_messages.items[2].message.parts.as_slice(),
        [bt_core::MessagePart::Text { text }] if text == "second"
    ));

    let latest_commands = store
        .load_branch_operator_commands_page(session.session_id, child.branch_id, None, 2)
        .expect("latest command page");
    assert!(latest_commands.has_more_before);
    assert_eq!(latest_commands.items.len(), 2);
    assert_eq!(latest_commands.items[0].raw_input, "/three");
    assert_eq!(latest_commands.items[1].raw_input, "/child");

    let older_commands = store
        .load_branch_operator_commands_page(
            session.session_id,
            child.branch_id,
            latest_commands.oldest_seq_id,
            10,
        )
        .expect("older command page");
    assert!(!older_commands.has_more_before);
    let older_inputs = older_commands
        .items
        .into_iter()
        .map(|command| command.raw_input)
        .collect::<Vec<_>>();
    assert_eq!(older_inputs, vec!["/one", "/two"]);
}

#[test]
fn session_history_search_returns_canonical_references() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    let turn_id = TurnId::new();
    let call_id = ToolCallId::new("call-search-1");

    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::User, "build a browser tic-tac-toe game"),
            },
        ))
        .expect("append message");
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::OperatorCommandRecorded {
                command_type: "slash_command".to_owned(),
                raw_input: "/history".to_owned(),
                output: "history includes tic-tac-toe setup".to_owned(),
                success: true,
            },
        ))
        .expect("append command");
    let tool_request = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Tool,
        EventPayload::ToolCallRequested {
            call_id: call_id.clone(),
            tool_name: "read".to_owned(),
            arguments: serde_json::json!({"path": "docs/architecture/overview.md"}),
        },
    )
    .with_turn_id(turn_id);
    store
        .append_event(&tool_request)
        .expect("append tool request");
    store
        .append_event(
            &EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Tool,
                EventPayload::ToolApprovalResolved {
                    call_id: call_id.clone(),
                    tool_name: "read".to_owned(),
                    request_fingerprint: Some("read:telemetry-approval".to_owned()),
                    resolution: None,
                    decision: ApprovalDecision::Approved {
                        decided_at: OffsetDateTime::now_utc(),
                        decided_by: "bt-tui".to_owned(),
                        scope: ApprovalScope::Session,
                        source: ApprovalDecisionSource::Human,
                    },
                },
            )
            .with_turn_id(turn_id),
        )
        .expect("append approval resolution");
    let tool_result = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Tool,
        EventPayload::ToolExecutionFinished {
            call_id: call_id.clone(),
            tool_name: "read".to_owned(),
            result: ToolResultEnvelope {
                call_id: call_id.clone(),
                tool_name: "read".to_owned(),
                is_error: false,
                output: serde_json::json!({"content": "architecture overview mentions telemetry"}),
                duration_ms: Some(7),
            },
        },
    )
    .with_turn_id(turn_id);
    store
        .append_event(&tool_result)
        .expect("append tool result");

    let matches = store
        .search_session_history(session.session_id, Some(branch.branch_id), "telemetry", 10)
        .expect("search");

    assert_eq!(matches.len(), 2);
    assert!(matches.iter().any(|entry| {
        entry.session_id == session.session_id
            && entry.branch_id == branch.branch_id
            && entry.turn_id == Some(turn_id)
            && entry.tool_call_id == Some(call_id.clone())
            && entry.matched_field == "result"
            && entry.snippet.contains("telemetry")
    }));

    let message_matches = store
        .search_session_history(
            session.session_id,
            Some(branch.branch_id),
            "tic-tac-toe",
            10,
        )
        .expect("message search");
    assert_eq!(message_matches.len(), 2);
    assert!(
        message_matches
            .iter()
            .any(|entry| entry.role == Some(Role::User))
    );
    assert!(
        message_matches
            .iter()
            .any(|entry| entry.command_type.as_deref() == Some("slash_command"))
    );
    let approval_matches = store
        .search_session_history(session.session_id, Some(branch.branch_id), "bt-tui", 10)
        .expect("approval search");
    assert_eq!(approval_matches.len(), 1);
    assert_eq!(approval_matches[0].tool_call_id, Some(call_id.clone()));
    assert_eq!(approval_matches[0].matched_field, "approval");

    for index in 0..32 {
        store
            .append_event(&EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Agent,
                EventPayload::MessageAppended {
                    message: Message::text(Role::Assistant, format!("irrelevant event {index}")),
                },
            ))
            .expect("append filler message");
    }
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::Assistant, "late durable-history marker"),
            },
        ))
        .expect("append late marker");

    let late_matches = store
        .search_session_history(
            session.session_id,
            Some(branch.branch_id),
            "durable-history",
            1,
        )
        .expect("late search");
    assert_eq!(late_matches.len(), 1);
    assert!(late_matches[0].snippet.contains("durable-history"));
}

#[test]
fn session_history_search_can_scope_to_branch_lineage_or_all_branches() {
    let mut store = SqliteSessionStore::open_in_memory().expect("store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let parent_before = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::MessageAppended {
            message: Message::text(Role::User, "shared parent clue"),
        },
    );
    let parent_after = EventEnvelope::new(
        session.session_id,
        branch.branch_id,
        SpanKind::Agent,
        EventPayload::MessageAppended {
            message: Message::text(Role::Assistant, "discarded parent clue"),
        },
    );
    store
        .append_event(&parent_before)
        .expect("append parent before");
    store
        .append_event(&parent_after)
        .expect("append parent after");

    let child = BranchRecord {
        branch_id: bt_core::BranchId::new(),
        session_id: session.session_id,
        parent_branch_id: Some(branch.branch_id),
        parent_event_id: Some(parent_before.event_id),
        head_event_id: None,
        summary: None,
        created_at: OffsetDateTime::now_utc(),
        is_default: false,
    };
    store.create_branch(&child).expect("create child branch");
    store
        .append_event(&EventEnvelope::new(
            session.session_id,
            child.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::User, "child only clue"),
            },
        ))
        .expect("append child");

    let child_lineage_matches = store
        .search_session_history(session.session_id, Some(child.branch_id), "clue", 10)
        .expect("child lineage search");
    assert!(
        child_lineage_matches
            .iter()
            .any(|entry| entry.snippet.contains("shared parent clue"))
    );
    assert!(
        child_lineage_matches
            .iter()
            .any(|entry| entry.snippet.contains("child only clue"))
    );
    assert!(
        !child_lineage_matches
            .iter()
            .any(|entry| entry.snippet.contains("discarded parent clue")),
        "child branch search must respect the fork boundary"
    );

    let root_branch_matches = store
        .search_session_history(session.session_id, Some(branch.branch_id), "child only", 10)
        .expect("root branch search");
    assert!(
        root_branch_matches.is_empty(),
        "branch-scoped search must not include sibling or child branches"
    );

    let all_branch_matches = store
        .search_session_history(session.session_id, None, "child only", 10)
        .expect("all branch search");
    assert_eq!(all_branch_matches.len(), 1);
    assert_eq!(all_branch_matches[0].branch_id, child.branch_id);
}
