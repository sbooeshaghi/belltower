//! Tool execution adapters and server-side tool registry construction.
//!
//! Runtime turn orchestration calls through these adapters for provider auth and
//! tool execution. This module wires server dependencies without owning the turn
//! loop or canonical session state.

use super::{ApiError, AppState};
use crate::agent_tools::register_agent_tools;
use crate::inspection_tools::register_inspection_tools;
use crate::session_search_tool::register_session_search_tool;
use crate::session_tools::register_session_tools;
use bt_auth::{CredentialResolver, require_runtime_credential_fresh};
use bt_core::{
    ApprovalDecision, ApprovalRequirement, ConnectionDescriptor, Message, MessagePart,
    SessionRecord, ToolCall, ToolContext, ToolResultEnvelope, ToolSpec, TurnId,
};
use bt_providers::provider_for_connection as build_provider_for_connection;
use bt_runtime::{TurnAdapterFuture, TurnExecutionAdapters};
use bt_tools::{BuiltInToolRegistry, CatalogueTool, WebSearchCredential};
use camino::{Utf8Path, Utf8PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use tracing::{Instrument, info_span};

pub(super) async fn resolve_tool_after_approval(
    state: &AppState,
    session: &SessionRecord,
    branch_id: bt_core::BranchId,
    turn_id: Option<TurnId>,
    tool_call: &ToolCall,
    decision: ApprovalDecision,
) -> Result<ToolResultEnvelope, ApiError> {
    match decision {
        ApprovalDecision::Approved { .. } => {
            execute_tool_call(state, session, branch_id, turn_id, tool_call).await
        }
        ApprovalDecision::Denied { reason, .. } => Ok(ToolResultEnvelope {
            call_id: bt_core::ToolCallId::new(tool_call.call_id.clone()),
            tool_name: tool_call.tool_name.clone(),
            is_error: true,
            output: serde_json::json!({
                "error": format!("tool `{}` was denied", tool_call.tool_name),
                "reason": reason,
            }),
            duration_ms: None,
        }),
    }
}

pub(super) async fn execute_tool_call(
    state: &AppState,
    session: &SessionRecord,
    branch_id: bt_core::BranchId,
    turn_id: Option<TurnId>,
    tool_call: &ToolCall,
) -> Result<ToolResultEnvelope, ApiError> {
    let tool_span = tool_scope_span(session, branch_id, turn_id, tool_call);
    async {
        let tools = build_tool_registry(state, session, branch_id).await?;
        let tool = tools.get(&tool_call.tool_name).ok_or_else(|| {
            bt_core::BelltowerError::Tool(format!("unknown tool `{}`", tool_call.tool_name))
        })?;
        if let Some(result) = validation_preflight_warning(state, session, branch_id, tool_call)? {
            return Ok(result);
        }
        Ok(tool
            .execute(
                tool_call_execution_arguments(tool_call),
                ToolContext {
                    project_root: session.project_root.clone(),
                },
            )
            .await?)
    }
    .instrument(tool_span)
    .await
}

pub(super) fn tool_execution_error_result(
    tool_call: &ToolCall,
    error: &bt_core::BelltowerError,
) -> ToolResultEnvelope {
    ToolResultEnvelope {
        call_id: bt_core::ToolCallId::new(tool_call.call_id.clone()),
        tool_name: tool_call.tool_name.clone(),
        is_error: true,
        output: serde_json::json!({
            "command": tool_call.arguments.get("command"),
            "error": {
                "class": error.class(),
                "code": error.code(),
                "message": error.to_string(),
                "retryable": error.retryable(),
            }
        }),
        duration_ms: None,
    }
}

fn validation_preflight_warning(
    state: &AppState,
    session: &SessionRecord,
    branch_id: bt_core::BranchId,
    tool_call: &ToolCall,
) -> Result<Option<ToolResultEnvelope>, ApiError> {
    if !matches!(tool_call.tool_name.as_str(), "write" | "edit") {
        return Ok(None);
    }

    let validation_surfaces = bt_context::validation_surfaces_for_project(&session.project_root);
    if validation_surfaces.is_empty() {
        return Ok(None);
    }

    let messages = state
        .runtime
        .messages(session.session_id, Some(branch_id))?;
    let required_surfaces = required_validation_surfaces_for_mutation(&validation_surfaces);
    if validation_surfaces_were_inspected(&session.project_root, &required_surfaces, &messages) {
        return Ok(None);
    }

    let unread = required_surfaces.into_iter().take(8).collect::<Vec<_>>();
    Ok(Some(validation_preflight_warning_result(
        &tool_call.call_id,
        &tool_call.tool_name,
        unread,
    )))
}

fn required_validation_surfaces_for_mutation(validation_surfaces: &[String]) -> Vec<String> {
    let check_surfaces = validation_surfaces
        .iter()
        .filter(|surface| is_check_validation_surface(surface))
        .cloned()
        .collect::<Vec<_>>();
    if check_surfaces.is_empty() {
        validation_surfaces.to_vec()
    } else {
        check_surfaces
    }
}

fn is_check_validation_surface(surface: &str) -> bool {
    let lower = surface.to_ascii_lowercase();
    lower.contains("test")
        || lower.contains("spec")
        || lower.contains("expected")
        || lower.contains("golden")
        || lower.contains("schema")
        || lower.contains("fixture")
}

fn validation_surfaces_were_inspected(
    project_root: &Utf8Path,
    validation_surfaces: &[String],
    messages: &[Message],
) -> bool {
    messages.iter().any(|message| {
        message.parts.iter().any(|part| {
            let MessagePart::ToolResult { result } = part else {
                return false;
            };
            result.tool_name == "read"
                && result_path_relative_to_root(project_root, &result.output).is_some_and(|path| {
                    validation_surfaces.iter().any(|surface| {
                        path == *surface || surface.ends_with('/') && path.starts_with(surface)
                    })
                })
        })
    })
}

fn validation_surfaces_were_inspected_in_results(
    project_root: &Utf8Path,
    validation_surfaces: &[String],
    results: &[ToolResultEnvelope],
) -> bool {
    results.iter().any(|result| {
        result.tool_name == "read"
            && result_path_relative_to_root(project_root, &result.output).is_some_and(|path| {
                validation_surfaces.iter().any(|surface| {
                    path == *surface || surface.ends_with('/') && path.starts_with(surface)
                })
            })
    })
}

fn result_path_relative_to_root(
    project_root: &Utf8Path,
    output: &serde_json::Value,
) -> Option<String> {
    let path = output.get("path")?.as_str()?;
    let path = camino::Utf8Path::new(path);
    let relative = path.strip_prefix(project_root).ok()?;
    let rendered = relative.as_str();
    (!rendered.is_empty()).then(|| rendered.to_owned())
}

#[derive(Clone)]
struct ValidationPreflightContext {
    project_root: Utf8PathBuf,
    required_surfaces: Arc<Vec<String>>,
    prior_messages: Arc<Vec<Message>>,
    turn_results: Arc<Mutex<Vec<ToolResultEnvelope>>>,
}

impl ValidationPreflightContext {
    fn allows_mutation(&self) -> bool {
        if self.required_surfaces.is_empty() {
            return true;
        }
        if validation_surfaces_were_inspected(
            &self.project_root,
            &self.required_surfaces,
            &self.prior_messages,
        ) {
            return true;
        }
        let Ok(turn_results) = self.turn_results.lock() else {
            return false;
        };
        validation_surfaces_were_inspected_in_results(
            &self.project_root,
            &self.required_surfaces,
            &turn_results,
        )
    }

    fn record_result(&self, result: &ToolResultEnvelope) {
        let Ok(mut turn_results) = self.turn_results.lock() else {
            return;
        };
        turn_results.push(result.clone());
    }

    fn warning_result(&self, tool_call_id: &str, tool_name: &str) -> ToolResultEnvelope {
        let unread = self
            .required_surfaces
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>();
        validation_preflight_warning_result(tool_call_id, tool_name, unread)
    }
}

struct ValidationPreflightTool {
    inner: Arc<dyn bt_core::traits::ToolExecutor>,
    context: ValidationPreflightContext,
}

impl bt_core::traits::ToolExecutor for ValidationPreflightTool {
    fn spec(&self) -> ToolSpec {
        self.inner.spec()
    }

    fn approval_requirement(&self, arguments: &serde_json::Value) -> ApprovalRequirement {
        self.inner.approval_requirement(arguments)
    }

    fn execute(
        &self,
        arguments: serde_json::Value,
        context: ToolContext,
    ) -> bt_core::traits::ToolFuture<'_> {
        Box::pin(async move {
            let spec = self.inner.spec();
            let call_id = arguments
                .get("call_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            if matches!(spec.name.as_str(), "write" | "edit") && !self.context.allows_mutation() {
                return Ok(self.context.warning_result(call_id, &spec.name));
            }

            let result = self.inner.execute(arguments, context).await?;
            if spec.name == "read" {
                self.context.record_result(&result);
            }
            Ok(result)
        })
    }
}

fn validation_preflight_warning_result(
    tool_call_id: &str,
    tool_name: &str,
    unread: Vec<String>,
) -> ToolResultEnvelope {
    ToolResultEnvelope {
        call_id: bt_core::ToolCallId::new(tool_call_id),
        tool_name: tool_name.to_owned(),
        is_error: true,
        output: serde_json::json!({
            "error": "validation check surfaces detected but not inspected before mutation",
            "validation_surfaces_unread": unread,
            "recovery": "Read the relevant validation surface files, then retry the write/edit with the validated artifact.",
        }),
        duration_ms: None,
    }
}

pub(super) fn session_scope_span(
    session: &SessionRecord,
    branch_id: bt_core::BranchId,
) -> tracing::Span {
    info_span!(
        "session",
        session_id = %session.session_id,
        branch_id = %branch_id,
        connection_id = %session.connection_id,
        project_root = %session.project_root,
    )
}

fn tool_scope_span(
    session: &SessionRecord,
    branch_id: bt_core::BranchId,
    turn_id: Option<TurnId>,
    tool_call: &ToolCall,
) -> tracing::Span {
    info_span!(
        "tool_execution",
        session_id = %session.session_id,
        branch_id = %branch_id,
        turn_id = ?turn_id,
        tool_name = %tool_call.tool_name,
        call_id = %tool_call.call_id,
        project_root = %session.project_root,
    )
}

fn tool_call_execution_arguments(tool_call: &ToolCall) -> serde_json::Value {
    match tool_call.arguments.clone() {
        serde_json::Value::Object(mut object) => {
            object.insert("call_id".to_owned(), serde_json::json!(tool_call.call_id));
            serde_json::Value::Object(object)
        }
        value => serde_json::json!({
            "call_id": tool_call.call_id,
            "input": value,
        }),
    }
}

pub(super) fn format_shell_command_output(result: &ToolResultEnvelope) -> String {
    let output = &result.output;
    let command = output
        .get("command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<unknown>");
    let status = output
        .get("status")
        .and_then(serde_json::Value::as_i64)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "timeout".to_owned());
    let stdout = output
        .get("stdout")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim_end();
    let stderr = output
        .get("stderr")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim_end();
    let mut lines = vec![format!("$ {command}"), format!("status: {status}")];
    if !stdout.is_empty() {
        lines.push("stdout:".to_owned());
        lines.push(stdout.to_owned());
    }
    if !stderr.is_empty() {
        lines.push("stderr:".to_owned());
        lines.push(stderr.to_owned());
    }
    if output
        .get("timed_out")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        lines.push("timed out".to_owned());
    }
    if let Some(error) = output.get("error") {
        let message = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("shell command failed");
        lines.push("error:".to_owned());
        lines.push(message.to_owned());
    }
    lines.join("\n")
}

pub(super) async fn build_tool_registry(
    state: &AppState,
    session: &SessionRecord,
    branch_id: bt_core::BranchId,
) -> Result<BuiltInToolRegistry, ApiError> {
    const CATALOGUE_THRESHOLD: usize = 10;

    let web_credentials = if session.tool_mode.is_extended() {
        web_search_credentials(state.runtime.config())
    } else {
        Vec::new()
    };
    let mut tools = BuiltInToolRegistry::new_with_config_and_web_credentials(
        session.tool_mode,
        state.runtime.config(),
        web_credentials,
    );
    if session.tool_mode.is_extended() {
        register_inspection_tools(
            &mut tools,
            state.runtime.clone(),
            session.session_id,
            branch_id,
        );
        register_session_search_tool(
            &mut tools,
            state.runtime.clone(),
            session.session_id,
            branch_id,
        );
        register_session_tools(
            &mut tools,
            state.runtime.clone(),
            session.session_id,
            branch_id,
        );
        register_agent_tools(&mut tools, state.clone(), session.session_id, branch_id);
    }
    for tool in state.runtime.mcp_registered_tools().await? {
        tools.register_arc(tool.executor);
    }
    install_validation_preflight_tools(&mut tools, state, session, branch_id)?;
    if tools.len() > CATALOGUE_THRESHOLD {
        let descriptors = tools.specs();
        tools.register(CatalogueTool::new(descriptors));
    }
    Ok(tools)
}

fn web_search_credentials(config: &bt_core::BelltowerConfig) -> Vec<WebSearchCredential> {
    let Ok(resolver) = CredentialResolver::new() else {
        return Vec::new();
    };
    config
        .web
        .backends
        .iter()
        .filter(|backend| backend.enabled)
        .filter_map(|backend| {
            let descriptor = backend.credential_descriptor();
            match resolver.resolve_api_key(&descriptor) {
                Ok(Some(credential)) => Some(WebSearchCredential {
                    backend_id: backend.id.clone(),
                    api_key: credential.secret,
                }),
                Ok(None) => None,
                Err(_) => None,
            }
        })
        .collect()
}

fn install_validation_preflight_tools(
    tools: &mut BuiltInToolRegistry,
    state: &AppState,
    session: &SessionRecord,
    branch_id: bt_core::BranchId,
) -> Result<(), ApiError> {
    let validation_surfaces = bt_context::validation_surfaces_for_project(&session.project_root);
    if validation_surfaces.is_empty() {
        return Ok(());
    }
    let required_surfaces = required_validation_surfaces_for_mutation(&validation_surfaces);
    if required_surfaces.is_empty() {
        return Ok(());
    }

    let context = ValidationPreflightContext {
        project_root: session.project_root.clone(),
        required_surfaces: Arc::new(required_surfaces),
        prior_messages: Arc::new(
            state
                .runtime
                .messages(session.session_id, Some(branch_id))?,
        ),
        turn_results: Arc::new(Mutex::new(Vec::new())),
    };

    for tool_name in ["read", "write", "edit"] {
        let Some(inner) = tools.get(tool_name) else {
            continue;
        };
        tools.register_arc(Arc::new(ValidationPreflightTool {
            inner,
            context: context.clone(),
        }));
    }
    Ok(())
}

#[derive(Clone)]
pub(super) struct ServerTurnAdapters {
    pub(super) state: AppState,
}

impl TurnExecutionAdapters for ServerTurnAdapters {
    fn build_tool_registry<'a>(
        &'a self,
        session: &'a SessionRecord,
        branch_id: bt_core::BranchId,
    ) -> TurnAdapterFuture<'a, BuiltInToolRegistry> {
        Box::pin(async move {
            build_tool_registry(&self.state, session, branch_id)
                .await
                .map_err(|error| error.0)
        })
    }

    fn provider_for_connection<'a>(
        &'a self,
        connection: &'a ConnectionDescriptor,
    ) -> TurnAdapterFuture<'a, Arc<dyn bt_core::traits::Provider>> {
        Box::pin(async move {
            let credential =
                require_runtime_credential_fresh(&CredentialResolver::new()?, connection).await?;
            build_provider_for_connection(connection, credential)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ValidationPreflightContext;
    use super::required_validation_surfaces_for_mutation;
    use super::validation_surfaces_were_inspected;
    use bt_core::{Message, MessagePart, Role, ToolCallId, ToolResultEnvelope};
    use camino::Utf8PathBuf;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    #[test]
    fn validation_preflight_recognizes_read_validation_surface() {
        let project_root = Utf8PathBuf::from("/tmp/project");
        let messages = vec![Message::from_part(
            Role::Tool,
            MessagePart::ToolResult {
                result: ToolResultEnvelope {
                    call_id: ToolCallId::new("call-read"),
                    tool_name: "read".to_owned(),
                    is_error: false,
                    output: json!({
                        "path": "/tmp/project/tests/test_outputs.py",
                        "binary": false,
                        "content": "assert output",
                    }),
                    duration_ms: None,
                },
            },
        )];

        assert!(validation_surfaces_were_inspected(
            &project_root,
            &["tests/test_outputs.py".to_owned()],
            &messages,
        ));
        assert!(validation_surfaces_were_inspected(
            &project_root,
            &["tests/".to_owned()],
            &messages,
        ));
    }

    #[test]
    fn validation_preflight_ignores_non_validation_reads() {
        let project_root = Utf8PathBuf::from("/tmp/project");
        let messages = vec![Message::from_part(
            Role::Tool,
            MessagePart::ToolResult {
                result: ToolResultEnvelope {
                    call_id: ToolCallId::new("call-read"),
                    tool_name: "read".to_owned(),
                    is_error: false,
                    output: json!({
                        "path": "/tmp/project/data.csv",
                        "binary": false,
                        "content": "date,value",
                    }),
                    duration_ms: None,
                },
            },
        )];

        assert!(!validation_surfaces_were_inspected(
            &project_root,
            &["tests/test_outputs.py".to_owned()],
            &messages,
        ));
    }

    #[test]
    fn validation_preflight_requires_check_surfaces_when_present() {
        let project_root = Utf8PathBuf::from("/tmp/project");
        let messages = vec![Message::from_part(
            Role::Tool,
            MessagePart::ToolResult {
                result: ToolResultEnvelope {
                    call_id: ToolCallId::new("call-read"),
                    tool_name: "read".to_owned(),
                    is_error: false,
                    output: json!({
                        "path": "/tmp/project/README.md",
                        "binary": false,
                        "content": "instructions",
                    }),
                    duration_ms: None,
                },
            },
        )];
        let required = required_validation_surfaces_for_mutation(&[
            "README.md".to_owned(),
            "tests/test_outputs.py".to_owned(),
        ]);

        assert_eq!(required, vec!["tests/test_outputs.py".to_owned()]);
        assert!(!validation_surfaces_were_inspected(
            &project_root,
            &required,
            &messages,
        ));
    }

    #[test]
    fn validation_preflight_tracks_reads_from_current_turn() {
        let project_root = Utf8PathBuf::from("/tmp/project");
        let context = ValidationPreflightContext {
            project_root: project_root.clone(),
            required_surfaces: Arc::new(vec!["tests/test_outputs.py".to_owned()]),
            prior_messages: Arc::new(Vec::new()),
            turn_results: Arc::new(Mutex::new(Vec::new())),
        };

        assert!(!context.allows_mutation());
        context.record_result(&ToolResultEnvelope {
            call_id: ToolCallId::new("call-read"),
            tool_name: "read".to_owned(),
            is_error: false,
            output: json!({
                "path": "/tmp/project/tests/test_outputs.py",
                "binary": false,
                "content": "assert output",
            }),
            duration_ms: None,
        });
        assert!(context.allows_mutation());
    }
}
