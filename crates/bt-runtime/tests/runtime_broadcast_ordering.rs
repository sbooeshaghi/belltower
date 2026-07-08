#![forbid(unsafe_code)]

use bt_core::{BelltowerConfig, ConnectionId, EventPayload, SessionToolMode};
use bt_runtime::BelltowerRuntime;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use tokio::sync::broadcast::error::TryRecvError;

#[test]
fn runtime_broadcast_waits_for_store_commit() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let database_path = temp_dir.path().join("sessions.sqlite");
    let runtime = Arc::new(BelltowerRuntime::open(config, &database_path).expect("runtime"));
    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("broadcast-order".to_owned()),
            None,
        )
        .expect("session creation");
    let mut receiver = runtime.subscribe();

    let lock_connection = rusqlite::Connection::open(&database_path).expect("lock connection");
    lock_connection
        .execute_batch("BEGIN IMMEDIATE;")
        .expect("hold sqlite writer lock");

    let (result_tx, result_rx) = mpsc::channel();
    let runtime_for_writer = Arc::clone(&runtime);
    let writer = thread::spawn(move || {
        let result = runtime_for_writer.record_operator_command(
            session.session_id,
            branch.branch_id,
            "test".to_owned(),
            "/test".to_owned(),
            "output".to_owned(),
            true,
        );
        result_tx.send(result).expect("send result");
    });

    thread::sleep(Duration::from_millis(100));
    assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
    assert!(matches!(
        result_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));

    lock_connection
        .execute_batch("COMMIT;")
        .expect("release sqlite writer lock");
    let seq_id = result_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("writer result")
        .expect("append succeeds after lock release");
    writer.join().expect("writer thread");

    let broadcast_event = loop {
        match receiver.try_recv() {
            Ok(event) if event.seq_id == Some(seq_id) => break event,
            Ok(_) => {}
            Err(TryRecvError::Empty) => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("unexpected broadcast error: {error}"),
        }
    };
    assert_eq!(broadcast_event.seq_id, Some(seq_id));
    assert!(matches!(
        broadcast_event.payload,
        EventPayload::OperatorCommandRecorded { .. }
    ));
    assert!(
        runtime
            .all_events(session.session_id)
            .expect("stored events")
            .iter()
            .any(|event| event.seq_id == Some(seq_id)
                && matches!(event.payload, EventPayload::OperatorCommandRecorded { .. }))
    );
}

#[test]
fn runtime_branch_activation_broadcasts_committed_event() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let database_path = temp_dir.path().join("sessions.sqlite");
    let runtime = BelltowerRuntime::open(config, &database_path).expect("runtime");
    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("branch-activation".to_owned()),
            None,
        )
        .expect("session creation");
    let child = runtime
        .create_branch(
            session.session_id,
            branch.branch_id,
            None,
            false,
            Some("child".to_owned()),
        )
        .expect("child branch");
    let mut receiver = runtime.subscribe();

    runtime
        .activate_branch(session.session_id, child.branch_id)
        .expect("activate branch");

    let event = receiver.try_recv().expect("activation broadcast");
    assert!(event.seq_id.is_some());
    assert!(matches!(
        event.payload,
        EventPayload::BranchActivated { branch_id } if branch_id == child.branch_id
    ));
    assert_eq!(
        runtime
            .default_branch(session.session_id)
            .expect("default branch")
            .expect("branch exists")
            .branch_id,
        child.branch_id
    );
    assert!(
        runtime
            .all_events(session.session_id)
            .expect("stored events")
            .iter()
            .any(|stored| stored.seq_id == event.seq_id
                && matches!(
                    stored.payload,
                    EventPayload::BranchActivated { branch_id } if branch_id == child.branch_id
                ))
    );
}
