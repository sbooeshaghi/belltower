//! Operator control and named-operation route handlers.
//!
//! These routes record operator-visible control changes through canonical
//! runtime/session paths. General status/readiness routes and tool execution
//! internals stay in `main.rs`.

use super::{
    ApiError, ApiJson, AppState, execute_tool_call, format_shell_command_output,
    require_default_branch, require_session,
};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use bt_core::traits::ToolExecutor;
use bt_core::{SessionId, ToolCall, ToolOperationContext, ToolOperationInitiator};
use bt_protocol::{
    CancelSessionRequest, RecordOperatorCommandRequest, RunShellCommandRequest, SteerSessionRequest,
};
use bt_tools::ShellTool;

pub(super) async fn cancel_session(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    ApiJson(request): ApiJson<CancelSessionRequest>,
) -> Result<StatusCode, ApiError> {
    let session = require_session(&state, session_id)?;
    let branch = require_default_branch(&state, session_id)?;
    state.runtime.cancel_session(
        session.session_id,
        branch.branch_id,
        request.reason.unwrap_or_else(|| "cancelled".to_owned()),
    )?;
    Ok(StatusCode::ACCEPTED)
}

pub(super) async fn steer_session(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    ApiJson(request): ApiJson<SteerSessionRequest>,
) -> Result<StatusCode, ApiError> {
    let session = require_session(&state, session_id)?;
    let branch = require_default_branch(&state, session_id)?;
    state
        .runtime
        .steer_session(session.session_id, branch.branch_id, request.message)?;
    Ok(StatusCode::ACCEPTED)
}

pub(super) async fn record_operator_command(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    ApiJson(request): ApiJson<RecordOperatorCommandRequest>,
) -> Result<StatusCode, ApiError> {
    let session = require_session(&state, session_id)?;
    let branch = require_default_branch(&state, session_id)?;
    state.runtime.record_operator_command(
        session.session_id,
        branch.branch_id,
        request.command_type,
        request.raw_input,
        request.output,
        request.success,
    )?;
    Ok(StatusCode::ACCEPTED)
}

pub(super) async fn run_shell_command(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    ApiJson(request): ApiJson<RunShellCommandRequest>,
) -> Result<StatusCode, ApiError> {
    let session = require_session(&state, session_id)?;
    let branch = require_default_branch(&state, session_id)?;
    let call_id = bt_core::ToolCallId::new(format!("operator-{}", uuid::Uuid::new_v4()));
    let arguments = serde_json::json!({
        "command": request.command,
        "call_id": call_id,
        "timeout_seconds": request.timeout_seconds.unwrap_or(120),
    });
    let tool_call = ToolCall {
        call_id: call_id.to_string(),
        tool_name: "shell".to_owned(),
        arguments: arguments.clone(),
    };
    let shell_spec = ShellTool.spec();
    state.runtime.record_tool_call_requested_with_context(
        session.session_id,
        branch.branch_id,
        call_id,
        "shell".to_owned(),
        arguments.clone(),
        ToolOperationContext::from_tool_spec(ToolOperationInitiator::Human, &shell_spec),
        None,
    )?;
    let tool_result =
        execute_tool_call(&state, &session, branch.branch_id, None, &tool_call).await?;
    state.runtime.record_tool_execution(
        session.session_id,
        branch.branch_id,
        tool_result.clone(),
        None,
    )?;
    state.runtime.record_operator_command(
        session.session_id,
        branch.branch_id,
        "shell_command".to_owned(),
        request.raw_input,
        format_shell_command_output(&tool_result),
        !tool_result.is_error,
    )?;
    Ok(StatusCode::ACCEPTED)
}
