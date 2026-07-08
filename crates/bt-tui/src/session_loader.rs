use super::*;

pub(super) struct ChatSessionSelection {
    pub(super) session_id: SessionId,
    pub(super) branch_id: BranchId,
    pub(super) connection_id: ConnectionId,
    pub(super) project_root: String,
}

pub(super) async fn open_chat_session(
    client: &BelltowerClient,
    args: &ChatArgs,
    mode: ChatOpenMode,
) -> Result<ChatSessionSelection, Box<dyn Error>> {
    let fallback_project_root = resolved_project_root(args)?;

    if let Some(session_id) = args.session_id {
        let sessions = client.list_sessions().await?.sessions;
        let branch_id = active_or_default_branch_id(client, session_id).await?;
        let session = sessions
            .iter()
            .find(|session| session.session_id == session_id);
        let project_root = args
            .project_root
            .clone()
            .or_else(|| session.map(|session| session.project_root.as_str().to_owned()))
            .unwrap_or(fallback_project_root);
        let connection_id = args
            .connection
            .clone()
            .or_else(|| session.map(|session| session.connection_id.clone()))
            .unwrap_or(default_connection()?);
        return Ok(ChatSessionSelection {
            session_id,
            branch_id,
            connection_id,
            project_root,
        });
    }

    let project_root = fallback_project_root;
    let connection_id = args.connection.clone().unwrap_or(default_connection()?);

    if mode == ChatOpenMode::ResumePicker {
        let sessions = client.list_sessions().await?.sessions;
        let selected = pick_resume_session(&sessions, &project_root, args.connection.as_ref())?;
        let branch_id = active_or_default_branch_id(client, selected.session_id).await?;
        return Ok(ChatSessionSelection {
            session_id: selected.session_id,
            branch_id,
            connection_id: selected.connection_id.clone(),
            project_root: selected.project_root.to_string(),
        });
    }

    let created = client
        .create_session(&CreateSessionRequest {
            project_root: project_root.clone(),
            connection_id: connection_id.clone(),
            model_id: None,
            tool_mode: None,
            display_name: args.display_name.clone(),
            objective: args.objective.clone(),
            budget: None,
        })
        .await?;
    Ok(ChatSessionSelection {
        session_id: created.session.session_id,
        branch_id: created.branch.branch_id,
        connection_id,
        project_root,
    })
}

pub(super) fn default_connection() -> Result<ConnectionId, Box<dyn Error>> {
    Ok(BelltowerConfig::load(None)?.defaults.default_connection)
}

pub(super) async fn active_or_default_branch_id(
    client: &BelltowerClient,
    session_id: SessionId,
) -> Result<BranchId, Box<dyn Error>> {
    let inspection = client.inspect_session(session_id).await?;
    inspection
        .inspection
        .active_branch
        .as_ref()
        .map(|branch| branch.branch_id)
        .or_else(|| {
            inspection
                .inspection
                .branches
                .iter()
                .find(|branch| branch.is_default)
                .map(|branch| branch.branch_id)
        })
        .or_else(|| {
            inspection
                .inspection
                .branches
                .first()
                .map(|branch| branch.branch_id)
        })
        .ok_or_else(|| "session has no branches".into())
}

pub(super) async fn load_session_state(
    client: BelltowerClient,
    session: SessionRecord,
) -> std::result::Result<LoadedSessionState, String> {
    let listed_sessions_fut = client.list_sessions();
    let inspection_fut = client.inspect_session(session.session_id);
    let connections_fut = client.connections();
    let (listed_sessions, inspection, connections) =
        tokio::try_join!(listed_sessions_fut, inspection_fut, connections_fut)
            .map_err(|error| error.to_string())?;
    let listed_sessions = listed_sessions.sessions;
    let inspection = inspection.inspection;
    let current_session = listed_sessions
        .iter()
        .find(|listed| listed.session_id == session.session_id)
        .cloned()
        .unwrap_or(inspection.session.clone());
    let branch_id = inspection
        .active_branch
        .as_ref()
        .map(|branch| branch.branch_id)
        .or_else(|| {
            inspection
                .branches
                .iter()
                .find(|branch| branch.is_default)
                .map(|branch| branch.branch_id)
        })
        .or_else(|| inspection.branches.first().map(|branch| branch.branch_id))
        .ok_or_else(|| "session has no branches".to_owned())?;
    let transcript =
        load_full_transcript(client.clone(), current_session.session_id, branch_id).await?;
    let connection_model = current_session.model_id.clone().or_else(|| {
        connections
            .connections
            .iter()
            .find(|connection| connection.id == current_session.connection_id)
            .map(|connection| connection.default_model.clone())
    });
    let sessions = sorted_sessions(&listed_sessions)
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();

    Ok(LoadedSessionState {
        session: current_session,
        branch_id,
        messages: transcript.messages,
        message_seq_ids: transcript.message_seq_ids,
        operator_commands: transcript.operator_commands,
        has_more_history_before: transcript.has_more_before,
        sessions,
        branches: inspection.branches,
        connections: connections.connections,
        connection_model,
        connection_models: Vec::new(),
        last_event_id: transcript.last_event_id.or(inspection.last_seq_id),
    })
}

pub(super) async fn load_full_transcript(
    client: BelltowerClient,
    session_id: SessionId,
    branch_id: BranchId,
) -> std::result::Result<LoadedTranscriptPage, String> {
    let mut combined = LoadedTranscriptPage {
        messages: Vec::new(),
        message_seq_ids: Vec::new(),
        operator_commands: Vec::new(),
        has_more_before: false,
        last_event_id: None,
    };
    let mut before_seq = None;

    loop {
        let page = load_transcript_page(client.clone(), session_id, branch_id, before_seq).await?;
        if combined.last_event_id.is_none() {
            combined.last_event_id = page.last_event_id;
        }

        let oldest_message = page.message_seq_ids.iter().flatten().copied().min();
        let oldest_command = page
            .operator_commands
            .iter()
            .filter_map(|command| command.seq_id)
            .min();
        let oldest_seq = match (oldest_message, oldest_command) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(left), None) => Some(left),
            (None, Some(right)) => Some(right),
            (None, None) => None,
        };
        let has_more_before = page.has_more_before;
        let messages = page.messages;
        let message_seq_ids = page.message_seq_ids;
        let operator_commands = page.operator_commands;
        merge_messages(
            &mut combined.messages,
            &mut combined.message_seq_ids,
            messages,
            message_seq_ids,
            None,
        );
        merge_operator_commands(&mut combined.operator_commands, operator_commands);
        if !has_more_before {
            combined.has_more_before = false;
            break;
        }
        let Some(next_before_seq) = oldest_seq else {
            combined.has_more_before = false;
            break;
        };
        before_seq = Some(next_before_seq);
    }

    Ok(combined)
}

pub(super) async fn load_transcript_page(
    client: BelltowerClient,
    session_id: SessionId,
    branch_id: BranchId,
    before_seq: Option<i64>,
) -> std::result::Result<LoadedTranscriptPage, String> {
    let messages_fut =
        client.branch_messages_page(session_id, branch_id, before_seq, TRANSCRIPT_PAGE_LIMIT);
    let commands_fut = client.branch_operator_commands_page(
        session_id,
        branch_id,
        before_seq,
        TRANSCRIPT_PAGE_LIMIT,
    );
    let (messages, commands) =
        tokio::try_join!(messages_fut, commands_fut).map_err(|error| error.to_string())?;

    Ok(LoadedTranscriptPage {
        messages: messages
            .messages
            .iter()
            .map(|entry| entry.message.clone())
            .collect(),
        message_seq_ids: messages
            .messages
            .iter()
            .map(|entry| Some(entry.seq_id))
            .collect(),
        operator_commands: commands
            .commands
            .into_iter()
            .map(|command| RecordedOperatorCommand {
                seq_id: Some(command.seq_id),
                occurred_at: command.occurred_at,
                command_type: command.command_type,
                raw_input: command.raw_input,
                output: command.output,
                success: command.success,
            })
            .collect(),
        has_more_before: messages.has_more_before || commands.has_more_before,
        last_event_id: messages.last_seq_id.or(commands.last_seq_id),
    })
}

pub(super) fn resolved_project_root(args: &ChatArgs) -> Result<String, Box<dyn Error>> {
    if let Some(project_root) = &args.project_root {
        return Ok(project_root.clone());
    }
    Ok(std::env::current_dir()?.display().to_string())
}

pub(super) fn matching_sessions<'a>(
    sessions: &'a [SessionRecord],
    project_root: &str,
    connection_id: &ConnectionId,
) -> Vec<&'a SessionRecord> {
    let mut matching = sessions
        .iter()
        .filter(|session| {
            session.project_root.as_str() == project_root && &session.connection_id == connection_id
        })
        .collect::<Vec<_>>();
    matching.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    matching
}

pub(super) fn sorted_sessions(sessions: &[SessionRecord]) -> Vec<&SessionRecord> {
    let mut sorted = sessions.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    sorted
}

pub(super) fn resume_candidates<'a>(
    sessions: &'a [SessionRecord],
    project_root: &str,
    connection_id: Option<&ConnectionId>,
) -> (Vec<&'a SessionRecord>, bool) {
    let filtered = match connection_id {
        Some(connection_id) => matching_sessions(sessions, project_root, connection_id),
        None => {
            let mut matching = sessions
                .iter()
                .filter(|session| session.project_root.as_str() == project_root)
                .collect::<Vec<_>>();
            matching.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
            matching
        }
    };
    if filtered.is_empty() {
        (sorted_sessions(sessions), true)
    } else {
        (filtered, false)
    }
}

pub(super) fn pick_resume_session<'a>(
    sessions: &'a [SessionRecord],
    project_root: &str,
    connection_id: Option<&ConnectionId>,
) -> Result<&'a SessionRecord, Box<dyn Error>> {
    let (candidates, used_fallback) = resume_candidates(sessions, project_root, connection_id);
    if candidates.is_empty() {
        return Err("No sessions are available to resume.".into());
    }

    println!("Resume session:");
    if used_fallback {
        println!("No sessions matched the current project or connection. Showing all sessions.");
    }
    for (index, session) in candidates.iter().enumerate() {
        println!(
            "  {}. {} [{}]  {}  {}",
            index + 1,
            session_title(Some(session)),
            short_id_string(&session.session_id.to_string()),
            session.connection_id,
            truncate_path(session.project_root.as_str(), 72)
        );
    }
    print!("Selection [1]: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(candidates[0]);
    }
    if let Ok(index) = trimmed.parse::<usize>()
        && let Some(session) = candidates.get(index.saturating_sub(1))
    {
        return Ok(session);
    }
    candidates
        .iter()
        .find(|session| {
            session.session_id.to_string() == trimmed
                || short_id_string(&session.session_id.to_string()) == trimmed
        })
        .copied()
        .ok_or_else(|| format!("unknown resume selection `{trimmed}`").into())
}

pub(super) fn session_title(session: Option<&SessionRecord>) -> String {
    session
        .and_then(|session| session.display_name.clone())
        .unwrap_or_else(|| "session".to_owned())
}
