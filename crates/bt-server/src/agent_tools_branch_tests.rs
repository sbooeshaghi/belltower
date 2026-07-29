use super::*;
use bt_core::{BelltowerConfig, Role, SessionToolMode};
use tempfile::NamedTempFile;

fn context() -> ToolContext {
    ToolContext {
        project_root: "/tmp/project".into(),
        cancellation: None,
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

fn registry(state: AppState, session_id: SessionId, branch_id: BranchId) -> BuiltInToolRegistry {
    let mut registry = BuiltInToolRegistry::default();
    register_agent_tools(&mut registry, state, session_id, branch_id);
    registry
}

#[tokio::test]
async fn agent_reply_retains_the_original_branch_edge_after_default_changes() {
    let (state, parent, parent_branch, _file) = fixture();
    let (child, child_branch) = state
        .runtime
        .spawn_child_session(
            parent.session_id,
            parent_branch.branch_id,
            None,
            "seek a branch-specific answer".to_owned(),
            Some("branch-critic".to_owned()),
            None,
            None,
        )
        .expect("child session");
    let child_message_branch = state
        .runtime
        .create_branch(
            child.session_id,
            child_branch.branch_id,
            None,
            false,
            Some("message destination".to_owned()),
        )
        .expect("non-default child branch");

    for (session, branch, prompt) in [
        (&parent, &parent_branch, "coordinate on the root branch"),
        (
            &child,
            &child_message_branch,
            "answer on the addressed child branch",
        ),
    ] {
        assert!(matches!(
            state.runtime.admit_user_message(
                session,
                branch,
                bt_core::Message::text(Role::User, prompt),
            ),
            Ok(bt_runtime::UserMessageAdmission::Started(_))
        ));
    }

    let parent_tools = registry(state.clone(), parent.session_id, parent_branch.branch_id);
    let child_tools = registry(
        state.clone(),
        child.session_id,
        child_message_branch.branch_id,
    );
    let question = parent_tools
        .get("send_agent_message")
        .expect("parent send tool")
        .execute(
            json!({
                "call_id": "call-branch-question",
                "target_session_id": child.session_id,
                "target_branch_id": child_message_branch.branch_id,
                "kind": "question",
                "delivery_mode": "notify",
                "text": "Which branch received this question?"
            }),
            context(),
        )
        .await
        .expect("send question to non-default child branch");
    let question_id = question.output["message_id"]
        .as_str()
        .expect("question message id")
        .to_owned();

    let alternate_parent_branch = state
        .runtime
        .create_branch(
            parent.session_id,
            parent_branch.branch_id,
            None,
            true,
            Some("new parent default".to_owned()),
        )
        .expect("activate alternate parent branch");
    let answer = child_tools
        .get("send_agent_message")
        .expect("child send tool")
        .execute(
            json!({
                "call_id": "call-branch-answer",
                "target_session_id": parent.session_id,
                "target_branch_id": parent_branch.branch_id,
                "kind": "answer",
                "delivery_mode": "notify",
                "in_reply_to": question_id,
                "text": "The explicitly addressed non-default child branch."
            }),
            context(),
        )
        .await
        .expect("reply to the original parent branch");
    assert_eq!(
        answer.output["target_branch_id"],
        json!(parent_branch.branch_id)
    );
    let answer_record = state
        .runtime
        .related_session_messages(parent.session_id)
        .expect("parent mailbox")
        .into_iter()
        .find(|record| record.message.text == "The explicitly addressed non-default child branch.")
        .expect("answer record");
    assert_eq!(
        answer_record.message.source_branch_id,
        child_message_branch.branch_id
    );
    assert_eq!(
        answer_record.message.destination_branch_id,
        parent_branch.branch_id
    );

    let parent_event_count = state
        .runtime
        .all_events(parent.session_id)
        .expect("parent events before conflict")
        .len();
    let child_event_count = state
        .runtime
        .all_events(child.session_id)
        .expect("child events before conflict")
        .len();
    let error = child_tools
        .get("send_agent_message")
        .expect("child send tool")
        .execute(
            json!({
                "call_id": "call-conflicting-branch-answer",
                "target_session_id": parent.session_id,
                "target_branch_id": alternate_parent_branch.branch_id,
                "kind": "answer",
                "delivery_mode": "notify",
                "in_reply_to": question_id,
                "text": "This reply must not follow the new default."
            }),
            context(),
        )
        .await
        .expect_err("reply cannot redirect to the new parent default");
    assert!(error.to_string().contains("target_branch_id conflicts"));
    assert_eq!(
        state
            .runtime
            .all_events(parent.session_id)
            .expect("parent events after conflict")
            .len(),
        parent_event_count
    );
    assert_eq!(
        state
            .runtime
            .all_events(child.session_id)
            .expect("child events after conflict")
            .len(),
        child_event_count
    );
}

#[tokio::test]
async fn agent_message_requires_an_explicit_target_branch_without_mutation() {
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
    state
        .runtime
        .admit_user_message(
            &parent,
            &parent_branch,
            bt_core::Message::text(Role::User, "coordinate the proof"),
        )
        .expect("admit parent turn");
    let parent_event_count = state
        .runtime
        .all_events(parent.session_id)
        .expect("parent events before rejection")
        .len();
    let child_event_count = state
        .runtime
        .all_events(child.session_id)
        .expect("child events before rejection")
        .len();

    let error = registry(state.clone(), parent.session_id, parent_branch.branch_id)
        .get("send_agent_message")
        .expect("send tool")
        .execute(
            json!({
                "call_id": "call-missing-target-branch",
                "target_session_id": child.session_id,
                "text": "do not infer a destination branch"
            }),
            context(),
        )
        .await
        .expect_err("target branch must be explicit");
    assert!(
        error
            .to_string()
            .contains("missing string argument `target_branch_id`")
    );
    assert_eq!(
        state
            .runtime
            .all_events(parent.session_id)
            .expect("parent events after rejection")
            .len(),
        parent_event_count
    );
    assert_eq!(
        state
            .runtime
            .all_events(child.session_id)
            .expect("child events after rejection")
            .len(),
        child_event_count
    );
}
