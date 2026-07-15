use bt_core::{
    BranchRecord, ConnectionId, EventEnvelope, EventPayload, Message, Role, SessionRecord,
    SessionStatus, SessionToolMode, SpanKind, TurnId, TurnStartSource,
    default_settings_revision_id,
};
use bt_session::{SessionTurnAdmission, SqliteSessionStore};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;
use time::OffsetDateTime;

#[test]
fn concurrent_append_event_allocates_unique_monotonic_seq_ids() {
    let temp_dir = TempDir::new().expect("tempdir");
    let database_path = temp_dir.path().join("sessions.sqlite");
    let mut store = SqliteSessionStore::open(&database_path).expect("open store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let writer_count = 16;
    let stores = (0..writer_count)
        .map(|_| SqliteSessionStore::open(&database_path).expect("open writer store"))
        .collect::<Vec<_>>();
    let barrier = Arc::new(Barrier::new(writer_count));

    let handles = stores
        .into_iter()
        .enumerate()
        .map(|(writer_index, mut writer_store)| {
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                let event = EventEnvelope::new(
                    session.session_id,
                    branch.branch_id,
                    SpanKind::Agent,
                    EventPayload::MessageAppended {
                        message: Message::text(Role::User, format!("writer-{writer_index}")),
                    },
                );
                writer_store.append_event(&event).expect("append event")
            })
        })
        .collect::<Vec<_>>();

    let mut returned_seq_ids = handles
        .into_iter()
        .map(|handle| handle.join().expect("writer thread"))
        .collect::<Vec<_>>();
    returned_seq_ids.sort_unstable();

    let expected_seq_ids = (1..=writer_count as i64).collect::<Vec<_>>();
    assert_eq!(returned_seq_ids, expected_seq_ids);

    let stored_seq_ids = store
        .load_session_event_log(session.session_id)
        .expect("load event log")
        .iter()
        .map(|stored| stored.seq_id)
        .collect::<Vec<_>>();
    assert_eq!(stored_seq_ids, expected_seq_ids);
    assert!(stored_seq_ids.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn concurrent_turn_admission_starts_once_and_queues_once() {
    let temp_dir = TempDir::new().expect("tempdir");
    let database_path = temp_dir.path().join("sessions.sqlite");
    let mut store = SqliteSessionStore::open(&database_path).expect("open store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");

    let writer_count = 2;
    let stores = (0..writer_count)
        .map(|_| SqliteSessionStore::open(&database_path).expect("open writer store"))
        .collect::<Vec<_>>();
    let barrier = Arc::new(Barrier::new(writer_count));
    let handles = stores
        .into_iter()
        .enumerate()
        .map(|(writer_index, mut writer_store)| {
            let barrier = Arc::clone(&barrier);
            let turn_id = TurnId::new();
            let message = format!("writer-{writer_index}");
            let started_events = vec![
                EventEnvelope::new(
                    session.session_id,
                    branch.branch_id,
                    SpanKind::Agent,
                    EventPayload::MessageAppended {
                        message: Message::text(Role::User, message.clone()),
                    },
                )
                .with_turn_id(turn_id),
                turn_started_event(&session, &branch, turn_id),
            ];
            let queued_events = vec![EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Agent,
                EventPayload::SessionQueuedMessageEnqueued {
                    message: Message::text(Role::User, message),
                    settings_revision_id: session.settings_revision_id,
                },
            )];
            let cancel_clear_event = EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Agent,
                EventPayload::SessionCancelCleared {
                    reason: "superseded by direct user operation".to_owned(),
                },
            );
            thread::spawn(move || {
                barrier.wait();
                writer_store
                    .admit_turn_or_queue(
                        session.session_id,
                        session.settings_revision_id,
                        &cancel_clear_event,
                        &started_events,
                        &queued_events,
                    )
                    .expect("admit turn or queue")
            })
        })
        .collect::<Vec<_>>();

    let admissions = handles
        .into_iter()
        .map(|handle| handle.join().expect("writer thread"))
        .collect::<Vec<_>>();
    assert_eq!(
        admissions
            .iter()
            .filter(|admission| matches!(admission, SessionTurnAdmission::Started { .. }))
            .count(),
        1
    );
    assert_eq!(
        admissions
            .iter()
            .filter(|admission| {
                matches!(admission, SessionTurnAdmission::Queued { position: 1, .. })
            })
            .count(),
        1
    );

    let events = store
        .load_session_event_log(session.session_id)
        .expect("load event log");
    assert_eq!(
        events
            .iter()
            .filter(|stored| matches!(stored.event.payload, EventPayload::TurnStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|stored| {
                matches!(
                    stored.event.payload,
                    EventPayload::SessionQueuedMessageEnqueued { .. }
                )
            })
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|stored| matches!(stored.event.payload, EventPayload::MessageAppended { .. }))
            .count(),
        1
    );
}

#[test]
fn turn_admission_waits_for_a_short_lived_sqlite_writer() {
    let temp_dir = TempDir::new().expect("tempdir");
    let database_path = temp_dir.path().join("sessions.sqlite");
    let mut store = SqliteSessionStore::open(&database_path).expect("open store");
    let (session, branch) = sample_session();
    store
        .create_session(&session, &branch)
        .expect("create session");
    let mut writer = SqliteSessionStore::open(&database_path).expect("open writer");
    let lock = rusqlite::Connection::open(&database_path).expect("lock connection");
    lock.execute_batch("BEGIN IMMEDIATE")
        .expect("hold writer lock");

    let turn_id = TurnId::new();
    let handle = thread::spawn(move || {
        let started_events = vec![
            EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Agent,
                EventPayload::MessageAppended {
                    message: Message::text(Role::User, "wait for lock"),
                },
            )
            .with_turn_id(turn_id),
            turn_started_event(&session, &branch, turn_id),
        ];
        let queued_events = vec![EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::SessionQueuedMessageEnqueued {
                message: Message::text(Role::User, "wait for lock"),
                settings_revision_id: session.settings_revision_id,
            },
        )];
        let cancel_clear_event = EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::SessionCancelCleared {
                reason: "superseded by direct user operation".to_owned(),
            },
        );
        writer
            .admit_turn_or_queue(
                session.session_id,
                session.settings_revision_id,
                &cancel_clear_event,
                &started_events,
                &queued_events,
            )
            .expect("admission waits for writer")
    });

    thread::sleep(Duration::from_millis(100));
    lock.execute_batch("COMMIT").expect("release writer lock");
    let admission = handle.join().expect("writer thread");
    assert!(matches!(admission, SessionTurnAdmission::Started { .. }));
}

fn turn_started_event(
    session: &SessionRecord,
    branch: &BranchRecord,
    turn_id: TurnId,
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
            settings_revision_id: session.settings_revision_id,
            source: TurnStartSource::UserMessage,
            resumed_from_call_id: None,
        },
    )
    .with_turn_id(turn_id)
}

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
