use bt_core::{
    BranchRecord, ConnectionId, EventEnvelope, EventPayload, Message, Role, SessionRecord,
    SessionStatus, SessionToolMode, SpanKind, default_settings_revision_id,
};
use bt_session::SqliteSessionStore;
use std::sync::{Arc, Barrier};
use std::thread;
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
