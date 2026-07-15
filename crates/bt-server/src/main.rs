#![forbid(unsafe_code)]

mod exports;
mod inspection_tools;
mod metadata_routes;
mod openapi;
mod operator_routes;
mod session_search_tool;
mod session_tools;
mod tool_execution;

use async_stream::stream;
use axum::extract::{FromRequest, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use bt_context::{SystemPromptBuilder, SystemPromptInput, summarize_messages};
#[cfg(test)]
use bt_core::Message;
use bt_core::{
    ApprovalRequest, BelltowerConfig, BranchRecord, ErrorClass, SessionId, SessionRecord,
    SessionRuntimeState, StartupTrace, ToolCallId, TurnId, config_dir,
    server_auth_token_path_for_url,
};
use bt_protocol::{
    ActivateBranchRequest, AnswerToolRequest, ApproveToolRequest, BranchMessagesPageResponse,
    BranchOperatorCommandsPageResponse, BranchOperatorCommandsResponse, BranchesResponse,
    CompactSessionRequest, CompactSessionResponse, ConnectionModelsResponse, ConnectionsResponse,
    ContextCompactionDto, CreateBranchRequest, CreateBranchResponse, CreateSessionRequest,
    CreateSessionResponse, ErrorEnvelope, ListSessionsResponse, McpInventoryResponse,
    McpServersResponse, McpToolsResponse, ModelBackendsResponse, ModelRecommendationsResponse,
    PROTOCOL_HEADER, PROTOCOL_VERSION, RawChunkDto, RawSseEnvelope, RecordedOperatorCommandDto,
    SendMessageOutcome, SendMessageRequest, SendMessageResponse, SequencedMessageDto,
    SessionBranchInspectionResponse, SessionEventsResponse, SessionExecutionResponse,
    SessionInspectionResponse, SessionLineageResponse, SessionMessagesResponse,
    SessionQueueClearResponse, SessionQueueResponse, SessionRawChunksResponse,
    SessionSearchResponse, SessionToolCallResponse, SessionTreeResponse, SessionTurnsResponse,
    SessionWorkflowResponse, SpawnSessionRequest, SpawnSessionResponse, StatusInspectionResponse,
    TurnRawChunksPageResponse, UpdateSessionBudgetRequest, UpdateSessionRequest,
    negotiate_protocol_version,
};
use bt_readiness::{inspect_connection_model_inventory, inspect_connection_models, inspect_status};
use bt_runtime::{BelltowerRuntime, TurnRunRequest};
use clap::Parser;
use serde::{Deserialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::convert::Infallible;
use std::fs;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tracing::Instrument;
use url::Url;

use crate::exports::{push_otlp_export, session_export};
use crate::metadata_routes::{health, server_info};
use crate::operator_routes::{
    cancel_session, record_operator_command, run_shell_command, steer_session,
};
use crate::session_tools::try_build_ask_response;
use crate::tool_execution::{
    ServerTurnAdapters, build_tool_registry, execute_tool_call, format_shell_command_output,
    resolve_tool_after_approval, session_scope_span, tool_execution_error_result,
};

const DEFAULT_TRANSCRIPT_PAGE_LIMIT: usize = 200;
const MAX_TRANSCRIPT_PAGE_LIMIT: usize = 500;
const DEFAULT_SESSION_SEARCH_LIMIT: usize = 20;
const MAX_SESSION_SEARCH_LIMIT: usize = 100;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long)]
    database: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SessionSearchQuery {
    query: String,
    branch_id: Option<bt_core::BranchId>,
    limit: Option<usize>,
}

#[derive(Clone)]
struct AppState {
    runtime: Arc<BelltowerRuntime>,
    token: Arc<String>,
}

struct ApiJson<T>(T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(
        request: axum::extract::Request,
        state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        Json::<T>::from_request(request, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(|rejection| {
                ApiError(bt_core::BelltowerError::Protocol(format!(
                    "invalid JSON request body: {rejection}"
                )))
            })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut trace = StartupTrace::from_env("bt-server");
    trace.mark("server_process.start");
    let args = Args::parse();
    trace.mark("config.load.start");
    let mut config = BelltowerConfig::load(None)?;
    trace.mark("config.load.done");
    if let Some(host) = args.host {
        config.server.host = host;
    }
    if let Some(port) = args.port {
        config.server.port = port;
    }
    fs::create_dir_all(config_dir())?;
    let server_url = Url::parse(&format!(
        "http://{}:{}/",
        config.server.host, config.server.port
    ))?;
    let token = uuid::Uuid::new_v4().to_string();
    let endpoint_auth_token_path = server_auth_token_path_for_url(&server_url)?;
    trace.mark("auth_token.write.start");
    write_auth_token(endpoint_auth_token_path.as_std_path(), &token)?;
    if config.server.auth_token_path != endpoint_auth_token_path {
        write_auth_token(&config.server.auth_token_path, &token)?;
    }
    trace.mark("auth_token.write.done");

    let database_path = args
        .database
        .unwrap_or_else(|| config_dir().join("belltower.sqlite").to_string());
    trace.mark("runtime.open.start");
    let runtime = Arc::new(BelltowerRuntime::open(config.clone(), database_path)?);
    trace.mark("runtime.open.done");
    let state = AppState {
        runtime,
        token: Arc::new(token),
    };

    let socket: SocketAddr = format!("{}:{}", config.server.host, config.server.port).parse()?;
    trace.mark("listener.bind.start");
    let listener = tokio::net::TcpListener::bind(socket).await?;
    trace.mark("listener.bind.done");
    trace.mark("server.ready");
    axum::serve(listener, build_app(state)).await?;
    Ok(())
}

fn build_app(state: AppState) -> Router {
    let protected = Router::new()
        .route("/sessions", post(create_session).get(list_sessions))
        .route(
            "/sessions/{session_id}",
            get(get_session).post(update_session),
        )
        .route("/sessions/{session_id}/budget", post(update_session_budget))
        .route("/sessions/{session_id}/message", post(send_message))
        .route("/sessions/{session_id}/spawn", post(spawn_session))
        .route("/sessions/{session_id}/approve", post(approve_tool))
        .route("/sessions/{session_id}/answer", post(answer_tool))
        .route(
            "/sessions/{session_id}/branches",
            get(session_branches).post(create_branch),
        )
        .route("/sessions/{session_id}/compact", post(compact_session))
        .route(
            "/sessions/{session_id}/branches/inspect",
            get(session_branch_inspection),
        )
        .route(
            "/sessions/{session_id}/branches/{branch_id}/activate",
            post(activate_branch),
        )
        .route(
            "/sessions/{session_id}/branches/{branch_id}/messages",
            get(branch_messages),
        )
        .route(
            "/sessions/{session_id}/branches/{branch_id}/messages/page",
            get(branch_messages_page),
        )
        .route(
            "/sessions/{session_id}/branches/{branch_id}/commands",
            get(branch_operator_commands),
        )
        .route(
            "/sessions/{session_id}/branches/{branch_id}/commands/page",
            get(branch_operator_commands_page),
        )
        .route("/sessions/{session_id}/raw_chunks", get(session_raw_chunks))
        .route(
            "/sessions/{session_id}/branches/{branch_id}/turns/{turn_id}/raw_chunks/page",
            get(turn_raw_chunks_page),
        )
        .route(
            "/sessions/{session_id}/export/{format}",
            get(session_export),
        )
        .route(
            "/sessions/{session_id}/export/otlp/push",
            post(push_otlp_export),
        )
        .route("/sessions/{session_id}/events", get(session_events))
        .route("/sessions/{session_id}/turns", get(session_turns))
        .route("/sessions/{session_id}/execution", get(session_execution))
        .route("/sessions/{session_id}/queue", get(session_queue))
        .route(
            "/sessions/{session_id}/queue/clear",
            post(clear_session_queue),
        )
        .route(
            "/sessions/{session_id}/tool-calls/{call_id}",
            get(session_tool_call),
        )
        .route("/sessions/{session_id}/search", get(session_search))
        .route("/sessions/{session_id}/lineage", get(session_lineage))
        .route("/sessions/{session_id}/workflow", get(session_workflow))
        .route("/sessions/{session_id}/tree", get(session_tree))
        .route(
            "/sessions/{session_id}/events/stream",
            get(stream_session_events),
        )
        .route("/sessions/{session_id}/messages", get(session_messages))
        .route("/sessions/{session_id}/cancel", post(cancel_session))
        .route("/sessions/{session_id}/steer", post(steer_session))
        .route(
            "/sessions/{session_id}/commands",
            post(record_operator_command),
        )
        .route(
            "/sessions/{session_id}/commands/shell",
            post(run_shell_command),
        )
        .route("/connections", get(connections))
        .route("/server/info", get(server_info))
        .route("/status/inspect", get(status_inspection))
        .route("/mcp", get(mcp_inventory))
        .route("/mcp/servers", get(mcp_servers))
        .route("/mcp/tools", get(mcp_tools))
        .route("/mcp/reload", post(reload_mcp))
        .route("/models/backends", get(model_backends))
        .route("/models/connections", get(connection_models))
        .route(
            "/models/connections/{connection_id}",
            get(connection_model_inventory),
        )
        .route("/models/recommendations", get(model_recommendations))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer_token,
        ));

    Router::new()
        .route("/health", get(health))
        .merge(protected)
        .with_state(state)
}

fn write_auth_token(path: impl AsRef<std::path::Path>, token: &str) -> std::io::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn not_found_error(resource: &str) -> bt_core::BelltowerError {
    bt_core::BelltowerError::NotFound(format!("{resource} not found"))
}

fn require_session(state: &AppState, session_id: SessionId) -> Result<SessionRecord, ApiError> {
    state
        .runtime
        .load_session(session_id)?
        .ok_or_else(|| not_found_error("session").into())
}

fn require_branch(
    state: &AppState,
    session_id: SessionId,
    branch_id: bt_core::BranchId,
    label: &str,
) -> Result<BranchRecord, ApiError> {
    state
        .runtime
        .load_branch(session_id, branch_id)?
        .ok_or_else(|| not_found_error(label).into())
}

fn require_default_branch(
    state: &AppState,
    session_id: SessionId,
) -> Result<BranchRecord, ApiError> {
    state.runtime.default_branch(session_id)?.ok_or_else(|| {
        bt_core::BelltowerError::InvalidState("default branch not found".to_owned()).into()
    })
}

async fn create_session(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<CreateSessionRequest>,
) -> Result<Json<CreateSessionResponse>, ApiError> {
    let budget = request.budget.clone();
    let (session, branch) = state.runtime.create_session(
        request.project_root.into(),
        request.connection_id,
        request.model_id,
        request.tool_mode.unwrap_or_default(),
        request.display_name,
        request.objective,
    )?;
    if let Some(budget) = budget {
        state
            .runtime
            .configure_session_budget(session.session_id, branch.branch_id, budget)?;
    }
    Ok(Json(CreateSessionResponse { session, branch }))
}

async fn update_session(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    ApiJson(request): ApiJson<UpdateSessionRequest>,
) -> Result<Json<CreateSessionResponse>, ApiError> {
    let _ = require_session(&state, session_id)?;
    let session = state.runtime.update_session_settings(
        session_id,
        request.connection_id,
        if request.reset_model_to_default {
            Some(None)
        } else {
            request.model_id.map(Some)
        },
        request.tool_mode,
    )?;
    let branch = require_default_branch(&state, session_id)?;
    Ok(Json(CreateSessionResponse { session, branch }))
}

async fn update_session_budget(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    ApiJson(request): ApiJson<UpdateSessionBudgetRequest>,
) -> Result<Json<SessionInspectionResponse>, ApiError> {
    let _ = require_session(&state, session_id)?;
    let branch = require_default_branch(&state, session_id)?;
    state
        .runtime
        .configure_session_budget(session_id, branch.branch_id, request.budget)?;
    let inspection = state
        .runtime
        .inspect_session(session_id)?
        .ok_or_else(|| not_found_error("session"))?;
    Ok(Json(SessionInspectionResponse { inspection }))
}

async fn spawn_session(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    ApiJson(request): ApiJson<SpawnSessionRequest>,
) -> Result<Json<SpawnSessionResponse>, ApiError> {
    let _ = require_session(&state, session_id)?;
    let _ = require_branch(
        &state,
        session_id,
        request.parent_branch_id,
        "parent branch",
    )?;
    let parent_branch_id = request.parent_branch_id;
    let (child_session, child_branch) = state.runtime.spawn_child_session(
        session_id,
        parent_branch_id,
        request.parent_turn_id,
        request.objective,
        request.display_name,
        request.connection_id,
        request.model_id,
    )?;
    Ok(Json(SpawnSessionResponse {
        parent_session_id: session_id,
        parent_branch_id,
        parent_turn_id: child_session.parent_turn_id,
        child_session,
        child_branch,
    }))
}

async fn get_session(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
) -> Result<Json<SessionInspectionResponse>, ApiError> {
    let inspection = state
        .runtime
        .inspect_session(session_id)?
        .ok_or_else(|| not_found_error("session"))?;
    Ok(Json(SessionInspectionResponse { inspection }))
}

async fn list_sessions(
    State(state): State<AppState>,
) -> Result<Json<ListSessionsResponse>, ApiError> {
    let sessions = state.runtime.list_sessions()?;
    Ok(Json(ListSessionsResponse { sessions }))
}

async fn send_message(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    ApiJson(request): ApiJson<SendMessageRequest>,
) -> Result<(StatusCode, Json<SendMessageResponse>), ApiError> {
    let session = require_session(&state, session_id)?;
    let branch = require_branch(&state, session_id, request.branch_id, "branch")?;
    state
        .runtime
        .ensure_session_budget_allows_turn(session.session_id)?;
    if let Some(queue) = state.runtime.inspect_queue(session_id)?
        && session_busy_for_queue(queue.runtime_state)
    {
        let position = state.runtime.queue_message(
            session.session_id,
            request.branch_id,
            request.message.clone(),
        )?;
        state.runtime.record_operator_command(
            session.session_id,
            request.branch_id,
            "queued_message".to_owned(),
            bt_core::render_queue_message_input(&request.message),
            format!("Queued at position {position}."),
            true,
        )?;
        return Ok((
            StatusCode::ACCEPTED,
            Json(SendMessageResponse {
                session_id,
                branch_id: request.branch_id,
                outcome: SendMessageOutcome::Queued { position },
            }),
        ));
    }
    let _ = state.runtime.clear_cancel_request(
        session.session_id,
        branch.branch_id,
        "cleared by direct user message",
    )?;
    state
        .runtime
        .append_raw_message(&session, &branch, request.message, None)?;
    run_session_turn(&state, &session, &branch).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(SendMessageResponse {
            session_id,
            branch_id: branch.branch_id,
            outcome: SendMessageOutcome::Dispatched,
        }),
    ))
}

async fn create_branch(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    ApiJson(request): ApiJson<CreateBranchRequest>,
) -> Result<Json<CreateBranchResponse>, ApiError> {
    let session = require_session(&state, session_id)?;
    let source_branch =
        require_branch(&state, session_id, request.from_branch_id, "source branch")?;

    let summary = if request.carry_summary && request.from_event_id.is_none() {
        summarize_branch_local_messages(&state, session_id, source_branch.branch_id)?
    } else {
        None
    };

    let branch = state.runtime.create_branch(
        session_id,
        source_branch.branch_id,
        request.from_event_id,
        request.activate,
        summary.as_ref().map(|summary| summary.summary.clone()),
    )?;

    if let Some(summary) = summary {
        state.runtime.record_branch_summary(
            session.session_id,
            branch.branch_id,
            summary.summary,
            summary.files_read,
            summary.files_modified,
        )?;
    }

    if request.activate {
        state
            .runtime
            .activate_branch(session_id, branch.branch_id)?;
    }

    Ok(Json(CreateBranchResponse { branch }))
}

async fn compact_session(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    ApiJson(request): ApiJson<CompactSessionRequest>,
) -> Result<Json<CompactSessionResponse>, ApiError> {
    let session = require_session(&state, session_id)?;
    let branch = require_branch(&state, session_id, request.branch_id, "branch")?;
    let tools = build_tool_registry(&state, &session, branch.branch_id).await?;
    let instructions = state
        .runtime
        .resolve_instructions(Some(&session.project_root))?;
    let plan = state
        .runtime
        .current_plan(session.session_id, branch.branch_id)?;
    let tool_specs = tools.specs();
    let system_prompt = Some(build_session_system_prompt(
        &state,
        &session,
        &tool_specs,
        &instructions,
        plan.as_ref(),
    )?);
    let prepared =
        state
            .runtime
            .compact_branch_context(&session, &branch, system_prompt, tool_specs, None)?;

    Ok(Json(CompactSessionResponse {
        session_id,
        branch_id: branch.branch_id,
        model_id: prepared.model_id,
        compaction: prepared.compaction.map(compaction_dto),
    }))
}

async fn approve_tool(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    ApiJson(request): ApiJson<ApproveToolRequest>,
) -> Result<StatusCode, ApiError> {
    let session = require_session(&state, session_id)?;
    let call_id = request.call_id.clone();
    let tool_name = request.tool_name.clone();
    let decision = request.decision.clone();
    let resumable = state
        .runtime
        .resumable_approval_call(session.session_id, call_id.clone(), &tool_name)?
        .ok_or_else(|| not_found_error("pending approval"))?;
    let branch = resumable.branch.clone();

    let session_span = session_scope_span(&session, branch.branch_id);
    async move {
        let approval_request = if let Some(snapshot) = &resumable.approval_request_snapshot {
            ApprovalRequest::from_snapshot_with_arguments(
                snapshot,
                resumable.tool_call.arguments.clone(),
            )
        } else {
            let tools = build_tool_registry(&state, &resumable.session, branch.branch_id).await?;
            let tool = tools.get(&tool_name).ok_or_else(|| {
                bt_core::BelltowerError::Unsupported(format!("unknown tool `{tool_name}`"))
            })?;
            let tool_spec = tool.spec();
            ApprovalRequest {
                session_id: session.session_id,
                call_id: call_id.clone(),
                tool_name: tool_name.clone(),
                arguments: resumable.tool_call.arguments.clone(),
                requirement: tool.approval_requirement(&resumable.tool_call.arguments),
                tool_metadata: tool_spec.metadata,
                requested_at: time::OffsetDateTime::now_utc(),
            }
        };
        let resumed = state.runtime.bootstrap_resumed_approval_turn(
            &resumable,
            &approval_request,
            request.decision,
        )?;
        let turn_session = state
            .runtime
            .session_for_settings_revision(&resumable.session, resumed.settings_revision_id)?;
        let connection = state
            .runtime
            .connection(&turn_session.connection_id)
            .ok_or_else(|| {
                bt_core::BelltowerError::Config(format!(
                    "connection `{}` is not configured",
                    turn_session.connection_id
                ))
            })?;
        let model_id = turn_session
            .model_id
            .clone()
            .unwrap_or_else(|| connection.default_model.clone());
        let started = Instant::now();
        let tool_result = match resolve_tool_after_approval(
            &state,
            &turn_session,
            branch.branch_id,
            Some(resumed.turn_id),
            &resumable.tool_call,
            decision,
        )
        .await
        {
            Ok(tool_result) => tool_result,
            Err(error) => {
                let latency_ms = started.elapsed().as_millis() as u64;
                state.runtime.record_resumed_tool_failure(
                    &turn_session,
                    &branch,
                    &connection,
                    &model_id,
                    resumed.turn_id,
                    &resumable.tool_call,
                    &error.0,
                    latency_ms,
                )?;
                return Err(error);
            }
        };
        let tool_result_message = bt_core::Message::from_part(
            bt_core::Role::Tool,
            bt_core::MessagePart::ToolResult {
                result: tool_result.clone(),
            },
        );
        if let Err(persistence_error) = state.runtime.record_tool_terminal_transition(
            turn_session.session_id,
            branch.branch_id,
            resumed.turn_id,
            tool_result.clone(),
            tool_result_message,
        ) {
            let latency_ms = started.elapsed().as_millis() as u64;
            state.runtime.record_turn_failure_transition(
                turn_session.session_id,
                branch.branch_id,
                &connection.provider,
                &model_id,
                resumed.turn_id,
                vec![tool_result],
                &persistence_error,
                latency_ms,
            )?;
            return Err(ApiError(persistence_error));
        }
        if state
            .runtime
            .has_pending_approvals_on_branch(turn_session.session_id, branch.branch_id)?
        {
            let latency_ms = started.elapsed().as_millis() as u64;
            state.runtime.record_turn_finished(
                turn_session.session_id,
                branch.branch_id,
                resumed.turn_id,
                connection.provider.clone(),
                model_id.clone(),
                "awaiting_approval".to_owned(),
                Some("tool_calls".to_owned()),
                latency_ms,
            )?;
            return Ok(StatusCode::ACCEPTED);
        }
        run_session_turn_with_turn_id(
            &state,
            &turn_session,
            &branch,
            Some(resumed.turn_id),
            true,
            resumed.settings_revision_id,
            bt_core::TurnStartSource::ApprovalResume,
            Some(call_id.clone()),
        )
        .await?;

        Ok(StatusCode::ACCEPTED)
    }
    .instrument(session_span)
    .await
}

async fn answer_tool(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    ApiJson(request): ApiJson<AnswerToolRequest>,
) -> Result<StatusCode, ApiError> {
    let session = require_session(&state, session_id)?;
    let resumable = state
        .runtime
        .resumable_input_call(session.session_id, request.call_id.clone())?
        .ok_or_else(|| not_found_error("pending ask tool call"))?;
    let branch = resumable.branch.clone();

    let session_span = session_scope_span(&session, branch.branch_id);
    async move {
        let tool_result = try_build_ask_response(&resumable.tool_call, request.response)?;
        let resumed = state
            .runtime
            .bootstrap_resumed_input_turn(&resumable, tool_result)?;
        run_session_turn_with_turn_id(
            &state,
            &session,
            &branch,
            Some(resumed.turn_id),
            true,
            resumed.settings_revision_id,
            bt_core::TurnStartSource::InputResume,
            Some(request.call_id.clone()),
        )
        .await?;
        Ok(StatusCode::ACCEPTED)
    }
    .instrument(session_span)
    .await
}

async fn activate_branch(
    State(state): State<AppState>,
    Path((session_id, branch_id)): Path<(SessionId, bt_core::BranchId)>,
    ApiJson(request): ApiJson<ActivateBranchRequest>,
) -> Result<StatusCode, ApiError> {
    let session = require_session(&state, session_id)?;
    let target_branch = require_branch(&state, session_id, branch_id, "target branch")?;
    let current_default = state.runtime.default_branch(session_id)?;

    if request.carry_summary
        && let Some(current_default) = current_default
        && current_default.branch_id != target_branch.branch_id
        && let Some(summary) =
            summarize_branch_local_messages(&state, session_id, current_default.branch_id)?
    {
        let merged = merge_branch_summary(target_branch.summary.clone(), &summary.summary);
        state.runtime.record_branch_summary(
            session.session_id,
            target_branch.branch_id,
            merged,
            summary.files_read,
            summary.files_modified,
        )?;
    }

    state
        .runtime
        .activate_branch(session_id, target_branch.branch_id)?;
    Ok(StatusCode::ACCEPTED)
}

async fn session_events(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    headers: HeaderMap,
) -> Result<Json<SessionEventsResponse>, ApiError> {
    let _ = require_session(&state, session_id)?;
    let last_event_id = parse_last_event_id(&headers);
    let events = state
        .runtime
        .replay_events_after(session_id, last_event_id, 500)?;
    let last_seq_id = state
        .runtime
        .inspect_session(session_id)?
        .and_then(|inspection| inspection.last_seq_id);
    Ok(Json(SessionEventsResponse {
        session_id,
        events,
        last_seq_id,
    }))
}

async fn stream_session_events(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let _ = require_session(&state, session_id)?;

    let last_event_id = parse_last_event_id(&headers);
    let mut receiver = state.runtime.subscribe();

    let stream = stream! {
        let mut last_seen = last_event_id;
        let mut replay_required = true;

        loop {
            if replay_required {
                loop {
                    let replay = match state.runtime.replay_events_after(session_id, last_seen, 500) {
                        Ok(replay) => replay,
                        Err(error) => {
                            yield Ok::<Event, Infallible>(sse_error_event_for(
                                last_seen.unwrap_or_default(),
                                ErrorClass::Runtime,
                                "replay_failed",
                                error.to_string(),
                                true,
                            ));
                            return;
                        }
                    };
                    let replay_len = replay.len();
                    for event in replay {
                        last_seen = event.seq_id.or(last_seen);
                        yield Ok::<Event, Infallible>(sse_event_for(event));
                    }
                    if replay_len < 500 {
                        break;
                    }
                }
                replay_required = false;
            }

            match receiver.recv().await {
                Ok(event) => {
                    if event.session_id != session_id {
                        continue;
                    }
                    if let (Some(seen), Some(seq_id)) = (last_seen, event.seq_id)
                        && seq_id <= seen
                    {
                        continue;
                    }
                    last_seen = event.seq_id.or(last_seen);
                    yield Ok::<Event, Infallible>(sse_event_for(event));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    replay_required = true;
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

async fn session_messages(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
) -> Result<Json<SessionMessagesResponse>, ApiError> {
    let _ = require_session(&state, session_id)?;
    let branch = state.runtime.default_branch(session_id)?;
    let messages = state
        .runtime
        .messages(session_id, branch.as_ref().map(|branch| branch.branch_id))?;
    Ok(Json(SessionMessagesResponse {
        session_id,
        branch_id: branch.map(|branch| branch.branch_id),
        messages,
    }))
}

async fn branch_messages(
    State(state): State<AppState>,
    Path((session_id, branch_id)): Path<(SessionId, bt_core::BranchId)>,
) -> Result<Json<SessionMessagesResponse>, ApiError> {
    let _ = require_session(&state, session_id)?;
    let _ = require_branch(&state, session_id, branch_id, "branch")?;
    let messages = state.runtime.messages(session_id, Some(branch_id))?;
    Ok(Json(SessionMessagesResponse {
        session_id,
        branch_id: Some(branch_id),
        messages,
    }))
}

fn transcript_page_limit(query: &HashMap<String, String>) -> usize {
    query
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_TRANSCRIPT_PAGE_LIMIT)
        .clamp(1, MAX_TRANSCRIPT_PAGE_LIMIT)
}

fn transcript_page_before_seq(query: &HashMap<String, String>) -> Option<i64> {
    query
        .get("before_seq")
        .and_then(|value| value.parse::<i64>().ok())
}

async fn branch_messages_page(
    State(state): State<AppState>,
    Path((session_id, branch_id)): Path<(SessionId, bt_core::BranchId)>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<BranchMessagesPageResponse>, ApiError> {
    let _ = require_session(&state, session_id)?;
    let _ = require_branch(&state, session_id, branch_id, "branch")?;
    let page = state.runtime.branch_messages_page(
        session_id,
        branch_id,
        transcript_page_before_seq(&query),
        transcript_page_limit(&query),
    )?;
    Ok(Json(BranchMessagesPageResponse {
        session_id,
        branch_id,
        messages: page
            .items
            .into_iter()
            .map(|message| SequencedMessageDto {
                seq_id: message.seq_id,
                message: message.message,
            })
            .collect(),
        oldest_seq_id: page.oldest_seq_id,
        newest_seq_id: page.newest_seq_id,
        has_more_before: page.has_more_before,
        last_seq_id: page.last_seq_id,
    }))
}

async fn branch_operator_commands(
    State(state): State<AppState>,
    Path((session_id, branch_id)): Path<(SessionId, bt_core::BranchId)>,
) -> Result<Json<BranchOperatorCommandsResponse>, ApiError> {
    let _ = require_session(&state, session_id)?;
    let _ = require_branch(&state, session_id, branch_id, "branch")?;
    let events = state
        .runtime
        .operator_command_events(session_id, branch_id)?;
    let last_seq_id = state
        .runtime
        .inspect_session(session_id)?
        .and_then(|inspection| inspection.last_seq_id);
    Ok(Json(BranchOperatorCommandsResponse {
        session_id,
        branch_id,
        events,
        last_seq_id,
    }))
}

async fn branch_operator_commands_page(
    State(state): State<AppState>,
    Path((session_id, branch_id)): Path<(SessionId, bt_core::BranchId)>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<BranchOperatorCommandsPageResponse>, ApiError> {
    let _ = require_session(&state, session_id)?;
    let _ = require_branch(&state, session_id, branch_id, "branch")?;
    let page = state.runtime.branch_operator_commands_page(
        session_id,
        branch_id,
        transcript_page_before_seq(&query),
        transcript_page_limit(&query),
    )?;
    Ok(Json(BranchOperatorCommandsPageResponse {
        session_id,
        branch_id,
        commands: page
            .items
            .into_iter()
            .map(|command| RecordedOperatorCommandDto {
                seq_id: command.seq_id,
                occurred_at: command.occurred_at,
                command_type: command.command_type,
                raw_input: command.raw_input,
                output: command.output,
                success: command.success,
            })
            .collect(),
        oldest_seq_id: page.oldest_seq_id,
        newest_seq_id: page.newest_seq_id,
        has_more_before: page.has_more_before,
        last_seq_id: page.last_seq_id,
    }))
}

async fn session_branches(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
) -> Result<Json<BranchesResponse>, ApiError> {
    let _ = require_session(&state, session_id)?;
    Ok(Json(BranchesResponse {
        session_id,
        branches: state.runtime.load_branches(session_id)?,
    }))
}

async fn session_branch_inspection(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
) -> Result<Json<SessionBranchInspectionResponse>, ApiError> {
    let inspection = state
        .runtime
        .session_tree(session_id)?
        .ok_or_else(|| not_found_error("session"))?;
    Ok(Json(SessionBranchInspectionResponse {
        session_id,
        active_branch_id: inspection.active_branch_id,
        branches: inspection.branches,
    }))
}

async fn session_turns(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
) -> Result<Json<SessionTurnsResponse>, ApiError> {
    let _ = require_session(&state, session_id)?;
    Ok(Json(SessionTurnsResponse {
        session_id,
        turns: state.runtime.turn_history(session_id)?,
    }))
}

async fn session_execution(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
) -> Result<Json<SessionExecutionResponse>, ApiError> {
    let inspection = state
        .runtime
        .session_execution(session_id)?
        .ok_or_else(|| not_found_error("session"))?;
    Ok(Json(SessionExecutionResponse { inspection }))
}

async fn session_queue(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
) -> Result<Json<SessionQueueResponse>, ApiError> {
    let inspection = state
        .runtime
        .inspect_queue(session_id)?
        .ok_or_else(|| not_found_error("session"))?;
    Ok(Json(SessionQueueResponse { inspection }))
}

async fn clear_session_queue(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
) -> Result<Json<SessionQueueClearResponse>, ApiError> {
    state
        .runtime
        .inspect_queue(session_id)?
        .ok_or_else(|| not_found_error("session"))?;
    let cleared_messages = state.runtime.clear_queued_messages(
        session_id,
        "Dropped by explicit queue clear.",
        "Dropped by explicit queue clear.",
        true,
    )?;
    Ok(Json(SessionQueueClearResponse {
        session_id,
        cleared_messages,
    }))
}

async fn session_tool_call(
    State(state): State<AppState>,
    Path((session_id, call_id)): Path<(SessionId, String)>,
) -> Result<Json<SessionToolCallResponse>, ApiError> {
    let _ = require_session(&state, session_id)?;
    let inspection = state
        .runtime
        .inspect_tool_call(session_id, ToolCallId::new(call_id))?
        .ok_or_else(|| not_found_error("tool call"))?;
    Ok(Json(SessionToolCallResponse { inspection }))
}

async fn session_search(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    Query(query): Query<SessionSearchQuery>,
) -> Result<Json<SessionSearchResponse>, ApiError> {
    let _ = require_session(&state, session_id)?;
    let query_text = query.query.trim().to_owned();
    if query_text.is_empty() {
        return Err(bt_core::BelltowerError::Protocol(
            "session search query must not be empty".to_owned(),
        )
        .into());
    }
    if let Some(branch_id) = query.branch_id {
        let _ = require_branch(&state, session_id, branch_id, "branch")?;
    }
    let limit = query
        .limit
        .unwrap_or(DEFAULT_SESSION_SEARCH_LIMIT)
        .clamp(1, MAX_SESSION_SEARCH_LIMIT);
    let results =
        state
            .runtime
            .search_session_history(session_id, query.branch_id, &query_text, limit)?;
    Ok(Json(SessionSearchResponse {
        session_id,
        branch_id: query.branch_id,
        query: query_text,
        limit,
        results,
    }))
}

async fn session_lineage(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
) -> Result<Json<SessionLineageResponse>, ApiError> {
    let inspection = state
        .runtime
        .session_lineage(session_id)?
        .ok_or_else(|| not_found_error("session"))?;
    Ok(Json(SessionLineageResponse { inspection }))
}

async fn session_workflow(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
) -> Result<Json<SessionWorkflowResponse>, ApiError> {
    let inspection = state
        .runtime
        .session_workflow(session_id)?
        .ok_or_else(|| not_found_error("session"))?;
    Ok(Json(SessionWorkflowResponse { inspection }))
}

async fn session_tree(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
) -> Result<Json<SessionTreeResponse>, ApiError> {
    let inspection = state
        .runtime
        .session_tree(session_id)?
        .ok_or_else(|| not_found_error("session"))?;
    Ok(Json(SessionTreeResponse { inspection }))
}

async fn session_raw_chunks(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
) -> Result<Json<SessionRawChunksResponse>, ApiError> {
    let _ = require_session(&state, session_id)?;
    let chunks = state
        .runtime
        .raw_chunks(session_id, 500)?
        .into_iter()
        .map(|chunk| {
            let received_at = chunk
                .received_at
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(|error| bt_core::BelltowerError::Storage(error.to_string()))?;
            Ok(RawChunkDto {
                chunk_id: chunk.chunk_id,
                branch_id: chunk.branch_id,
                turn_id: chunk.turn_id,
                llm_call_ordinal: chunk.llm_call_ordinal,
                event_id: chunk.event_id,
                provider: chunk.provider,
                stream_name: chunk.stream_name,
                content_base64: base64::engine::general_purpose::STANDARD.encode(chunk.content),
                received_at,
            })
        })
        .collect::<Result<Vec<_>, bt_core::BelltowerError>>()?;
    Ok(Json(SessionRawChunksResponse { session_id, chunks }))
}

fn raw_chunk_page_before_id(query: &HashMap<String, String>) -> Option<i64> {
    query
        .get("before_chunk_id")
        .and_then(|value| value.parse::<i64>().ok())
}

fn raw_chunk_page_llm_call_ordinal(query: &HashMap<String, String>) -> Option<u32> {
    query
        .get("llm_call_ordinal")
        .and_then(|value| value.parse::<u32>().ok())
}

async fn turn_raw_chunks_page(
    State(state): State<AppState>,
    Path((session_id, branch_id, turn_id)): Path<(SessionId, bt_core::BranchId, TurnId)>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<TurnRawChunksPageResponse>, ApiError> {
    let _ = require_session(&state, session_id)?;
    let _ = require_branch(&state, session_id, branch_id, "branch")?;
    let page = state.runtime.turn_raw_chunks_page(
        session_id,
        branch_id,
        turn_id,
        raw_chunk_page_llm_call_ordinal(&query),
        raw_chunk_page_before_id(&query),
        transcript_page_limit(&query),
    )?;
    let chunks = page
        .items
        .into_iter()
        .map(|chunk| {
            let received_at = chunk
                .received_at
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(|error| bt_core::BelltowerError::Storage(error.to_string()))?;
            Ok(RawChunkDto {
                chunk_id: chunk.chunk_id,
                branch_id: chunk.branch_id,
                turn_id: chunk.turn_id,
                llm_call_ordinal: chunk.llm_call_ordinal,
                event_id: chunk.event_id,
                provider: chunk.provider,
                stream_name: chunk.stream_name,
                content_base64: base64::engine::general_purpose::STANDARD.encode(chunk.content),
                received_at,
            })
        })
        .collect::<Result<Vec<_>, bt_core::BelltowerError>>()?;
    Ok(Json(TurnRawChunksPageResponse {
        session_id,
        branch_id,
        turn_id,
        chunks,
        oldest_chunk_id: page.oldest_chunk_id,
        newest_chunk_id: page.newest_chunk_id,
        has_more_before: page.has_more_before,
    }))
}

fn summarize_branch_local_messages(
    state: &AppState,
    session_id: SessionId,
    branch_id: bt_core::BranchId,
) -> Result<Option<bt_context::BranchSummary>, ApiError> {
    let messages = state.runtime.local_branch_messages(session_id, branch_id)?;
    Ok(summarize_messages(&messages, 2_048))
}

fn merge_branch_summary(existing: Option<String>, next: &str) -> String {
    match existing {
        Some(existing) if !existing.trim().is_empty() => {
            format!("{existing}\n\nAdditional branch handoff:\n{next}")
        }
        _ => next.to_owned(),
    }
}

fn compaction_dto(report: bt_runtime::ContextCompactionReport) -> ContextCompactionDto {
    ContextCompactionDto {
        compaction_id: report.compaction_id,
        trigger: report.trigger,
        phase: report.phase,
        status: report.status,
        reason: report.reason,
        provider: report.provider,
        model: report.model,
        context_boundary_seq_id: report.context_boundary_seq_id,
        summary_message_id: report.summary_message_id,
        first_kept_message_id: report.first_kept_message_id,
        first_kept_branch_id: report.first_kept_branch_id,
        first_kept_seq_id: report.first_kept_seq_id,
        latency_ms: report.latency_ms,
        summary: report.summary,
        messages_before: report.messages_before,
        messages_after: report.messages_after,
        tokens_before: report.tokens_before,
        tokens_after: report.tokens_after,
        files_read: report.files_read,
        files_modified: report.files_modified,
    }
}

async fn connections(State(state): State<AppState>) -> Result<Json<ConnectionsResponse>, ApiError> {
    Ok(Json(ConnectionsResponse {
        connections: state.runtime.connections(),
    }))
}

async fn status_inspection(
    State(state): State<AppState>,
) -> Result<Json<StatusInspectionResponse>, ApiError> {
    let inspection = inspect_status(state.runtime.config()).await?;
    Ok(Json(StatusInspectionResponse { inspection }))
}

async fn mcp_inventory(
    State(state): State<AppState>,
) -> Result<Json<McpInventoryResponse>, ApiError> {
    let inventory = state.runtime.mcp_inventory().await?;
    Ok(Json(McpInventoryResponse {
        servers: inventory.servers,
        tools: inventory.tools,
    }))
}

async fn mcp_servers(State(state): State<AppState>) -> Result<Json<McpServersResponse>, ApiError> {
    Ok(Json(McpServersResponse {
        servers: state.runtime.mcp_servers().await,
    }))
}

async fn mcp_tools(State(state): State<AppState>) -> Result<Json<McpToolsResponse>, ApiError> {
    Ok(Json(McpToolsResponse {
        tools: state.runtime.mcp_tools().await?,
    }))
}

async fn reload_mcp(State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    state.runtime.reload_mcp().await?;
    Ok(StatusCode::ACCEPTED)
}

async fn model_backends(
    State(state): State<AppState>,
) -> Result<Json<ModelBackendsResponse>, ApiError> {
    Ok(Json(ModelBackendsResponse {
        backends: state.runtime.model_backends().await,
    }))
}

async fn connection_models(
    State(state): State<AppState>,
) -> Result<Json<ConnectionModelsResponse>, ApiError> {
    Ok(Json(ConnectionModelsResponse {
        connections: inspect_connection_models(state.runtime.config()).await?,
    }))
}

async fn connection_model_inventory(
    State(state): State<AppState>,
    Path(connection_id): Path<bt_core::ConnectionId>,
) -> Result<Json<ConnectionModelsResponse>, ApiError> {
    Ok(Json(ConnectionModelsResponse {
        connections: vec![
            inspect_connection_model_inventory(state.runtime.config(), &connection_id).await?,
        ],
    }))
}

async fn model_recommendations(
    State(state): State<AppState>,
) -> Result<Json<ModelRecommendationsResponse>, ApiError> {
    Ok(Json(ModelRecommendationsResponse {
        report: state.runtime.model_recommendations()?,
    }))
}

async fn require_bearer_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let auth_ok = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == format!("Bearer {}", state.token.as_str()));
    if !auth_ok {
        return (
            StatusCode::UNAUTHORIZED,
            [(PROTOCOL_HEADER, HeaderValue::from_static(PROTOCOL_VERSION))],
            Json(ErrorEnvelope::new(
                ErrorClass::Auth,
                "unauthorized",
                "unauthorized",
                false,
            )),
        )
            .into_response();
    }

    let requested_version = headers
        .get(PROTOCOL_HEADER)
        .and_then(|value| value.to_str().ok());
    if let Err(error) = negotiate_protocol_version(requested_version) {
        return (
            StatusCode::BAD_REQUEST,
            [(PROTOCOL_HEADER, HeaderValue::from_static(PROTOCOL_VERSION))],
            Json(ErrorEnvelope {
                class: ErrorClass::Protocol,
                code: "unsupported_protocol_version".to_owned(),
                message: error.to_string(),
                retryable: false,
                details: None,
            }),
        )
            .into_response();
    }

    next.run(request).await
}

fn parse_last_event_id(headers: &HeaderMap) -> Option<i64> {
    headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
}

fn raw_sse_payload(id: i64, event_name: &str, data: serde_json::Value) -> String {
    serde_json::to_string(&RawSseEnvelope {
        id,
        event: event_name.to_owned(),
        protocol_version: PROTOCOL_VERSION.to_owned(),
        data,
    })
    .unwrap_or_else(|_| {
        format!(
            r#"{{"id":{id},"event":"error","protocol_version":"{PROTOCOL_VERSION}","data":{{"class":"protocol","code":"sse_serialization_error","message":"failed to serialize SSE envelope","retryable":false}}}}"#
        )
    })
}

fn sse_error_event_for(
    id: i64,
    class: ErrorClass,
    code: &str,
    message: String,
    retryable: bool,
) -> Event {
    let data = serde_json::to_value(ErrorEnvelope {
        class,
        code: code.to_owned(),
        message,
        retryable,
        details: None,
    })
    .unwrap_or_else(|_| {
        serde_json::json!({
            "class": "protocol",
            "code": "sse_serialization_error",
            "message": "failed to serialize SSE error envelope",
            "retryable": false,
        })
    });
    Event::default()
        .id(id.to_string())
        .event("error")
        .data(raw_sse_payload(id, "error", data))
}

fn sse_event_for(event: bt_core::EventEnvelope) -> Event {
    let seq_id = event.seq_id.unwrap_or_default();
    let event_name = event.kind().to_owned();
    let (wire_event_name, payload) = match serde_json::to_value(&event) {
        Ok(data) => (
            event_name.clone(),
            raw_sse_payload(seq_id, &event_name, data),
        ),
        Err(error) => {
            let data = serde_json::to_value(ErrorEnvelope {
                class: ErrorClass::Protocol,
                code: "sse_serialization_error".to_owned(),
                message: error.to_string(),
                retryable: false,
                details: None,
            })
            .unwrap_or_else(|_| {
                serde_json::json!({
                    "class": "protocol",
                    "code": "sse_serialization_error",
                    "message": "failed to serialize SSE error envelope",
                    "retryable": false,
                })
            });
            ("error".to_owned(), raw_sse_payload(seq_id, "error", data))
        }
    };

    Event::default()
        .id(seq_id.to_string())
        .event(wire_event_name)
        .data(payload)
}

async fn run_session_turn(
    state: &AppState,
    session: &SessionRecord,
    branch: &BranchRecord,
) -> Result<(), ApiError> {
    run_session_turn_with_turn_id(
        state,
        session,
        branch,
        None,
        false,
        session.settings_revision_id,
        bt_core::TurnStartSource::UserMessage,
        None,
    )
    .await
}

async fn run_session_turn_with_turn_id(
    state: &AppState,
    session: &SessionRecord,
    branch: &BranchRecord,
    initial_turn_id: Option<TurnId>,
    initial_turn_started: bool,
    initial_settings_revision_id: u64,
    initial_source: bt_core::TurnStartSource,
    resumed_from_call_id: Option<ToolCallId>,
) -> Result<(), ApiError> {
    let adapters = ServerTurnAdapters {
        state: state.clone(),
    };
    state
        .runtime
        .turn_orchestrator()
        .run_session_turns(
            &adapters,
            TurnRunRequest {
                session: session.clone(),
                branch: branch.clone(),
                initial_turn_id,
                initial_turn_started,
                initial_settings_revision_id,
                initial_source,
                resumed_from_call_id,
            },
        )
        .await?;
    Ok(())
}

#[cfg(test)]
fn queue_message_input(message: &Message) -> String {
    bt_core::render_queue_message_input(message)
}

fn session_busy_for_queue(runtime_state: SessionRuntimeState) -> bool {
    matches!(
        runtime_state,
        SessionRuntimeState::Working
            | SessionRuntimeState::WaitingOnInput
            | SessionRuntimeState::WaitingOnApproval
            | SessionRuntimeState::CancelRequested
    )
}

#[derive(Debug)]
struct ApiError(bt_core::BelltowerError);

impl<T> From<T> for ApiError
where
    T: Into<bt_core::BelltowerError>,
{
    fn from(value: T) -> Self {
        Self(value.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = status_for_error(&self.0);
        let envelope = ErrorEnvelope::from_core(&self.0);
        (
            status,
            [(PROTOCOL_HEADER, HeaderValue::from_static(PROTOCOL_VERSION))],
            Json(envelope),
        )
            .into_response()
    }
}

fn status_for_error(error: &bt_core::BelltowerError) -> StatusCode {
    match error.class() {
        ErrorClass::Config | ErrorClass::Tool => StatusCode::BAD_REQUEST,
        ErrorClass::Auth => StatusCode::UNAUTHORIZED,
        ErrorClass::Provider => StatusCode::BAD_GATEWAY,
        ErrorClass::Protocol => StatusCode::BAD_REQUEST,
        ErrorClass::NotFound => StatusCode::NOT_FOUND,
        ErrorClass::Unsupported => StatusCode::NOT_IMPLEMENTED,
        ErrorClass::Storage | ErrorClass::Runtime => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct BuiltSessionSystemPrompt {
    prompt: String,
    core_prompt: bt_core::InstructionDocument,
    provider_overlay: Option<bt_core::InstructionDocument>,
}

fn build_session_system_prompt(
    state: &AppState,
    session: &bt_core::SessionRecord,
    tools: &[bt_core::ToolSpec],
    instructions: &[bt_core::InstructionDocument],
    plan: Option<&bt_core::PlanInspection>,
) -> Result<String, ApiError> {
    Ok(
        build_session_system_prompt_with_provenance(state, session, tools, instructions, plan)?
            .prompt,
    )
}

fn build_session_system_prompt_with_provenance(
    state: &AppState,
    session: &bt_core::SessionRecord,
    tools: &[bt_core::ToolSpec],
    instructions: &[bt_core::InstructionDocument],
    plan: Option<&bt_core::PlanInspection>,
) -> Result<BuiltSessionSystemPrompt, ApiError> {
    let connection = state
        .runtime
        .connection(&session.connection_id)
        .ok_or_else(|| {
            bt_core::BelltowerError::InvalidState(format!(
                "connection `{}` not found",
                session.connection_id
            ))
        })?;
    let core_prompt = state.runtime.core_prompt_document();
    let provider_overlay = state.runtime.provider_prompt_overlay(&connection.provider);

    let prompt = SystemPromptBuilder::build(SystemPromptInput {
        session,
        provider_family: &connection.provider,
        core_prompt: &core_prompt,
        provider_overlay: provider_overlay.as_ref(),
        instructions,
        plan,
        tools,
    });

    Ok(BuiltSessionSystemPrompt {
        prompt,
        core_prompt,
        provider_overlay,
    })
}

#[cfg(test)]
mod tests;
