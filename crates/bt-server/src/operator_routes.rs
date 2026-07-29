//! Operator control and named-operation route handlers.
//!
//! These routes record operator-visible control changes through canonical
//! runtime/session paths. General status/readiness routes and tool execution
//! internals stay in `main.rs`.

use super::{
    ApiError, ApiJson, AppState, execute_tool_call, format_shell_command_output, require_branch,
    require_session, tool_execution_error_result,
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
    let branch = require_branch(&state, session_id, request.branch_id, "branch")?;
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
    let branch = require_branch(&state, session_id, request.branch_id, "branch")?;
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
    let branch = require_branch(&state, session_id, request.branch_id, "branch")?;
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
    let branch = require_branch(&state, session_id, request.branch_id, "branch")?;
    let call_id = bt_core::ToolCallId::new(format!("operator-{}", uuid::Uuid::new_v4()));
    let configured_timeout = state.runtime.config().approval.shell_timeout_seconds;
    let timeout_seconds = request.timeout_seconds.unwrap_or(configured_timeout);
    if timeout_seconds == 0 || timeout_seconds > configured_timeout {
        return Err(ApiError(bt_core::BelltowerError::Protocol(format!(
            "timeout_seconds must be between 1 and configured maximum {configured_timeout}"
        ))));
    }
    let arguments = serde_json::json!({
        "command": request.command,
        "call_id": call_id,
        "timeout_seconds": timeout_seconds,
    });
    let tool_call = ToolCall {
        call_id: call_id.to_string(),
        tool_name: "shell".to_owned(),
        arguments: arguments.clone(),
    };
    let shell_spec = ShellTool::new(configured_timeout).spec();
    state.runtime.record_tool_operation_request_transition(
        session.session_id,
        branch.branch_id,
        call_id.clone(),
        "shell".to_owned(),
        arguments.clone(),
        ToolOperationContext::from_tool_spec(ToolOperationInitiator::Human, &shell_spec),
        None,
    )?;
    match execute_tool_call(&state, &session, branch.branch_id, None, &tool_call).await {
        Ok(tool_result) => {
            let output = format_shell_command_output(&tool_result);
            let success = !tool_result.is_error;
            if let Err(persistence_error) = state.runtime.record_operator_tool_terminal_transition(
                session.session_id,
                branch.branch_id,
                tool_result.clone(),
                "shell_command".to_owned(),
                request.raw_input.clone(),
                output.clone(),
                success,
            ) {
                state.runtime.recover_operator_tool_terminal_transition(
                    session.session_id,
                    branch.branch_id,
                    tool_result,
                    "shell_command".to_owned(),
                    request.raw_input,
                    output,
                    success,
                    &persistence_error,
                )?;
                return Err(ApiError(persistence_error));
            }
            Ok(StatusCode::ACCEPTED)
        }
        Err(error) => {
            let tool_result = tool_execution_error_result(&tool_call, &error.0);
            let output = format_shell_command_output(&tool_result);
            if let Err(persistence_error) = state.runtime.record_operator_tool_terminal_transition(
                session.session_id,
                branch.branch_id,
                tool_result.clone(),
                "shell_command".to_owned(),
                request.raw_input.clone(),
                output.clone(),
                false,
            ) {
                state.runtime.recover_operator_tool_terminal_transition(
                    session.session_id,
                    branch.branch_id,
                    tool_result,
                    "shell_command".to_owned(),
                    request.raw_input,
                    output,
                    false,
                    &persistence_error,
                )?;
                return Err(ApiError(persistence_error));
            }
            Err(error)
        }
    }
}
