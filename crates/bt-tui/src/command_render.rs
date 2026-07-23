//! Command-output rendering for the TUI.
//!
//! This module renders operator-visible slash-command output that is recorded
//! back into session history. Telemetry and raw-event rendering live in the
//! `telemetry` child module.

mod telemetry;

pub(crate) use telemetry::{
    correlate_raw_diff_events, render_raw_diff_output, render_raw_output, render_usage_output,
    turn_event_matches,
};

use bt_core::{
    BelltowerConfig, BranchInspection, ConnectionAuthState, ConnectionId, ConnectionModelInventory,
    ConnectionReadinessInspection, ConnectionSupportState, McpServerDescriptor, McpServerStatus,
    McpToolDescriptor, McpTransportKind, ModelBackendDescriptor, ModelBackendStatus,
    ModelRecommendationsReport, RelatedSessionSummary, SessionExecutionInspection,
    SessionInspection, SessionLineageInspection, SessionQueueInspection, SessionRuntimeState,
    SessionStatus, SessionToolCallInspection, SessionToolMode, SessionTreeInspection,
    SessionWorkflowInspection, TraceEventCounts, TraceTurnInspection, TurnInspection,
    WorkflowRuntimeCounts, WorkflowSessionNode, WorkflowStatusCounts, config_path,
};
use bt_protocol::{
    HealthResponse, PushOtlpExportResponse, SessionQueueClearResponse, SpawnSessionResponse,
};

use crate::message_render::render_message_preview;
use crate::{render_tool_mode, short_id_string, truncate_detail, truncate_path};

use self::telemetry::sanitize_inline_preview;

pub(crate) fn render_tool_call_inspection_output(inspection: &SessionToolCallInspection) -> String {
    let mut lines = vec![
        format!("Tool call {} {}", inspection.tool_name, inspection.call_id),
        format!(
            "branch={} turn={}",
            inspection
                .branch_id
                .map(|branch_id| branch_id.to_string())
                .unwrap_or_else(|| "na".to_owned()),
            inspection
                .turn_id
                .map(|turn_id| turn_id.to_string())
                .unwrap_or_else(|| "na".to_owned())
        ),
        format!(
            "approval_status={} execution_status={}",
            inspection.approval_status.as_deref().unwrap_or("none"),
            inspection.execution_status.as_deref().unwrap_or("na")
        ),
        format!(
            "requested_seq={} completed_seq={}",
            inspection
                .requested_seq_id
                .map(|seq_id| seq_id.to_string())
                .unwrap_or_else(|| "na".to_owned()),
            inspection
                .completed_seq_id
                .map(|seq_id| seq_id.to_string())
                .unwrap_or_else(|| "na".to_owned())
        ),
    ];

    if let Some(arguments) = inspection.arguments.as_ref() {
        lines.push("Arguments:".to_owned());
        lines.push(
            serde_json::to_string_pretty(arguments).unwrap_or_else(|_| arguments.to_string()),
        );
    }

    if let Some(result) = inspection.result.as_ref() {
        lines.push("Result:".to_owned());
        lines.push(serde_json::to_string_pretty(result).unwrap_or_else(|_| result.to_string()));
    }

    if let Some(decision) = inspection.approval_decision.as_ref() {
        lines.push("Approval decision:".to_owned());
        lines.push(
            serde_json::to_string_pretty(decision).unwrap_or_else(|_| format!("{decision:?}")),
        );
    }

    lines.join("\n")
}

pub(crate) fn render_status_output(
    inspection: &bt_core::StatusInspection,
    current_connection: &bt_core::ConnectionId,
    current_model: &str,
    tool_mode: SessionToolMode,
) -> String {
    let mut lines = vec![
        format!(
            "Status current_connection={} current_model={} tool_mode={} default_connection={} auth_storage={}",
            current_connection,
            current_model,
            render_tool_mode(tool_mode),
            inspection.default_connection,
            inspection.auth_storage
        ),
        "Connections:".to_owned(),
    ];
    lines.extend(
        inspection
            .connections
            .iter()
            .flat_map(render_status_connection_lines),
    );
    lines.join("\n")
}

pub(crate) fn render_defaults_output(
    config: &BelltowerConfig,
    current_connection: &ConnectionId,
    current_model: &str,
) -> String {
    let default_connection = &config.defaults.default_connection;
    let default_model = config
        .connections
        .iter()
        .find(|connection| &connection.id == default_connection)
        .map(|connection| connection.default_model.as_str())
        .unwrap_or("unknown");

    [
        format!("Defaults config={}", config_path()),
        format!(
            "Global default_connection={} default_model={}",
            default_connection, default_model
        ),
        format!(
            "Current session connection={} model={}",
            current_connection, current_model
        ),
        "Notes:".to_owned(),
        "- /connection, /model, and /use only affect the current session".to_owned(),
        "- /defaults connection <id> updates the launcher default for future sessions".to_owned(),
        "- /defaults model <model-id> updates the current connection's future default model"
            .to_owned(),
        "- /defaults use <connection> [model] updates future-session defaults without changing this session"
            .to_owned(),
    ]
    .join("\n")
}

fn render_status_connection_lines(connection: &ConnectionReadinessInspection) -> Vec<String> {
    vec![
        format!(
            "- {} readiness={} model={} auth={}",
            connection.connection_id,
            connection.readiness_label(),
            connection.default_model,
            render_connection_auth_detail(connection),
        ),
        format!(
            "  probe={}",
            sanitize_inline_preview(&connection.probe_status)
        ),
    ]
}

fn render_doctor_connection_lines(connection: &ConnectionReadinessInspection) -> Vec<String> {
    let support = match connection.support_state {
        ConnectionSupportState::RuntimeSupported => "runtime_supported",
        ConnectionSupportState::Planned => "planned",
    };

    vec![
        format!(
            "- {} ({}) readiness={}",
            connection.connection_id,
            connection.provider,
            connection.readiness_label(),
        ),
        format!(
            "  ready={} support={} model={}",
            if connection.is_ready() { "yes" } else { "no" },
            support,
            connection.default_model,
        ),
        format!("  auth={}", render_connection_auth_detail(connection)),
        format!(
            "  probe={}",
            sanitize_inline_preview(&connection.probe_status)
        ),
    ]
}

fn render_connection_auth_detail(connection: &ConnectionReadinessInspection) -> String {
    match connection.auth_state {
        ConnectionAuthState::NotRequired => "not_required".to_owned(),
        ConnectionAuthState::Missing => "missing".to_owned(),
        ConnectionAuthState::Configured => match (&connection.auth_kind, &connection.auth_source) {
            (Some(kind), Some(source)) => {
                let tried = if connection.auth_tried_sources.is_empty() {
                    String::new()
                } else {
                    format!(":tried={}", connection.auth_tried_sources.join("->"))
                };
                format!(
                    "{}:{}{}",
                    render_credential_kind_label(kind),
                    sanitize_inline_preview(source),
                    tried,
                )
            }
            (Some(kind), None) => render_credential_kind_label(kind).to_owned(),
            _ => "configured".to_owned(),
        },
    }
}

fn render_credential_kind_label(kind: &bt_core::CredentialKind) -> &'static str {
    match kind {
        bt_core::CredentialKind::ApiKey => "secret",
        bt_core::CredentialKind::OAuthToken => "oauth",
        bt_core::CredentialKind::JsonDocument => "json",
    }
}

pub(crate) fn render_models_output(
    connections: &[ConnectionModelInventory],
    backends: &[ModelBackendDescriptor],
    report: &ModelRecommendationsReport,
    current_connection: &bt_core::ConnectionId,
    current_model: &str,
) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "Models current_connection={} current_model={}",
        current_connection, current_model
    ));
    let memory = report
        .hardware
        .total_memory_gb
        .map_or_else(|| "unknown".to_owned(), |value| format!("{value}GB"));
    lines.push(format!("Local models memory={memory}"));
    lines.push("Connections:".to_owned());
    lines.extend(connections.iter().flat_map(render_connection_models_lines));
    lines.push("Backends:".to_owned());
    lines.extend(backends.iter().map(render_model_backend_line));
    lines.push("Recommendations:".to_owned());
    lines.extend(report.recommendations.iter().map(|recommendation| {
        format!(
            "- {} fits={} max_memory={}GB models={}",
            recommendation.label,
            if recommendation.fits_hardware {
                "yes"
            } else {
                "no"
            },
            recommendation.max_memory_gb,
            recommendation.models.join(", ")
        )
    }));
    lines.join("\n")
}

fn render_connection_models_lines(connection: &ConnectionModelInventory) -> Vec<String> {
    let auth = match connection.auth_state {
        ConnectionAuthState::NotRequired => "not_required".to_owned(),
        ConnectionAuthState::Missing => "missing".to_owned(),
        ConnectionAuthState::Configured => connection
            .auth_source
            .as_deref()
            .map(sanitize_inline_preview)
            .unwrap_or_else(|| "configured".to_owned()),
    };
    let support = match connection.support_state {
        ConnectionSupportState::RuntimeSupported => "runtime_supported",
        ConnectionSupportState::Planned => "planned",
    };
    let discovered = connection.discovered_source.as_deref().unwrap_or("none");
    let models = if connection.models.is_empty() {
        "none".to_owned()
    } else {
        connection
            .models
            .iter()
            .map(|model| match model.source {
                bt_core::ConnectionModelSource::Default => format!("{} (default)", model.model_id),
                bt_core::ConnectionModelSource::Fallback => {
                    format!("{} (fallback)", model.model_id)
                }
                bt_core::ConnectionModelSource::Discovered => {
                    format!("{} (discovered)", model.model_id)
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };

    vec![
        format!(
            "- {} ({}) readiness={}",
            connection.connection_id,
            connection.provider,
            connection.readiness_label(),
        ),
        format!(
            "  support={} default_model={}",
            support, connection.default_model
        ),
        format!("  auth={} discovered_from={}", auth, discovered),
        format!("  models={}", sanitize_inline_preview(&models)),
    ]
}

pub(crate) fn render_doctor_output(
    health: &HealthResponse,
    inspection: &bt_core::StatusInspection,
    connections: &[ConnectionModelInventory],
    backends: &[ModelBackendDescriptor],
    mcp_servers: &[McpServerDescriptor],
    mcp_tools: &[McpToolDescriptor],
    current_connection: &bt_core::ConnectionId,
    current_model: &str,
    tool_mode: SessionToolMode,
) -> String {
    let mut lines = vec![
        format!(
            "Doctor status={} protocol={}",
            health.status, health.protocol_version
        ),
        format!(
            "Current connection={} model={} tool_mode={}",
            current_connection,
            current_model,
            render_tool_mode(tool_mode)
        ),
        format!(
            "Defaults default_connection={} auth_storage={}",
            inspection.default_connection, inspection.auth_storage
        ),
        "Connections:".to_owned(),
    ];
    lines.extend(
        inspection
            .connections
            .iter()
            .flat_map(render_doctor_connection_lines),
    );
    lines.push("Connection models:".to_owned());
    lines.extend(connections.iter().flat_map(render_connection_models_lines));
    lines.push("Local backends:".to_owned());
    lines.extend(backends.iter().map(render_model_backend_line));
    lines.push(render_mcp_summary_line(mcp_servers, mcp_tools));
    lines.push("MCP servers:".to_owned());
    lines.extend(
        mcp_servers
            .iter()
            .map(|server| render_mcp_server_line(server, mcp_tools)),
    );
    if !mcp_tools.is_empty() {
        lines.push("MCP tools:".to_owned());
        lines.extend(mcp_tools.iter().map(render_mcp_tool_line));
    }
    lines.join("\n")
}

pub(crate) fn render_mcp_output(
    servers: &[McpServerDescriptor],
    tools: &[McpToolDescriptor],
    reloaded: bool,
) -> String {
    let mut lines = Vec::new();
    if reloaded {
        lines.push("MCP reloaded.".to_owned());
    }
    lines.push(render_mcp_summary_line(servers, tools));
    lines.push("Servers:".to_owned());
    lines.extend(
        servers
            .iter()
            .map(|server| render_mcp_server_line(server, tools)),
    );
    if tools.is_empty() {
        lines.push("Tools: none".to_owned());
    } else {
        lines.push("Tools:".to_owned());
        lines.extend(tools.iter().map(render_mcp_tool_line));
    }
    lines.join("\n")
}

fn render_mcp_summary_line(servers: &[McpServerDescriptor], tools: &[McpToolDescriptor]) -> String {
    let discovered = servers
        .iter()
        .filter(|server| matches!(server.status, McpServerStatus::Discovered))
        .count();
    let ready = servers
        .iter()
        .filter(|server| matches!(server.status, McpServerStatus::Ready))
        .count();
    let degraded = servers
        .iter()
        .filter(|server| matches!(server.status, McpServerStatus::Degraded { .. }))
        .count();
    let disabled = servers.iter().filter(|server| !server.enabled).count();
    format!(
        "MCP servers={} discovered={} ready={} degraded={} disabled={} tools={}",
        servers.len(),
        discovered,
        ready,
        degraded,
        disabled,
        tools.len()
    )
}

fn render_mcp_server_line(server: &McpServerDescriptor, tools: &[McpToolDescriptor]) -> String {
    let transport = match server.transport {
        McpTransportKind::Stdio => "stdio",
        McpTransportKind::StreamableHttp => "http",
    };
    let status = match &server.status {
        McpServerStatus::Configured => "configured".to_owned(),
        McpServerStatus::Discovered => "discovered".to_owned(),
        McpServerStatus::Ready => "ready".to_owned(),
        McpServerStatus::Degraded { reason } => format!("degraded:{reason}"),
    };
    let tool_count = tools
        .iter()
        .filter(|tool| tool.server_name == server.name)
        .count();
    format!(
        "- {} enabled={} transport={} status={} tools={}",
        server.name,
        if server.enabled { "yes" } else { "no" },
        transport,
        status,
        tool_count
    )
}

fn render_mcp_tool_line(tool: &McpToolDescriptor) -> String {
    format!(
        "- {} server={} risk=high group=external",
        tool.qualified_name, tool.server_name
    )
}

fn render_model_backend_line(backend: &ModelBackendDescriptor) -> String {
    let status = match &backend.status {
        ModelBackendStatus::Ready => "ready".to_owned(),
        ModelBackendStatus::Disabled => "disabled".to_owned(),
        ModelBackendStatus::Unreachable { reason } => format!("unreachable:{reason}"),
    };
    let models = if backend.available_models.is_empty() {
        "models=0".to_owned()
    } else {
        format!("models={}", backend.available_models.join(", "))
    };
    format!("- {} status={} {}", backend.label, status, models)
}

pub(crate) fn render_turn_history_output(turns: &[TurnInspection], limit: usize) -> String {
    let mut lines = turns
        .iter()
        .map(render_turn_history_entry)
        .collect::<Vec<_>>();
    if lines.len() > limit {
        lines = lines.split_off(lines.len() - limit);
    }
    if lines.is_empty() {
        "No turns recorded yet.".to_owned()
    } else {
        lines.join("\n")
    }
}

pub(crate) fn render_export_output(
    format_label: &str,
    content_type: &str,
    content: &str,
    output_path: Option<&str>,
) -> String {
    let preview = preview_export_content(content, 1_200);
    let destination = output_path
        .map(|path| format!(" path={path}"))
        .unwrap_or_default();
    format!(
        "Export {} content_type={} bytes={}{}\nPreview:\n{}",
        format_label,
        content_type,
        content.len(),
        destination,
        preview,
    )
}

pub(crate) fn render_otlp_push_output(response: &PushOtlpExportResponse) -> String {
    let mut lines = vec![
        "OTLP collector upload complete.".to_owned(),
        format!("Request URL: {}", response.request_url),
        format!("Content-Type: {}", response.content_type),
        format!("Bytes sent: {}", response.bytes_sent),
        format!("HTTP status: {}", response.status_code),
    ];
    if let Some(rejected_spans) = response.rejected_spans {
        lines.push(format!("Rejected spans: {rejected_spans}"));
    }
    if let Some(warning) = &response.warning
        && !warning.trim().is_empty()
    {
        lines.push(format!("Warning: {warning}"));
    }
    lines.join("\n")
}

pub(crate) fn render_compaction_output(
    branch_label: String,
    model_id: &str,
    compaction: Option<&bt_protocol::ContextCompactionDto>,
) -> String {
    match compaction {
        Some(compaction) => {
            let files_read = if compaction.files_read.is_empty() {
                "none".to_owned()
            } else {
                compaction
                    .files_read
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let files_modified = if compaction.files_modified.is_empty() {
                "none".to_owned()
            } else {
                compaction
                    .files_modified
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            format!(
                "Compacted branch {} for model {}\nMessages: {} -> {}\nTokens: {} -> {}\nFiles read: {}\nFiles modified: {}\nSummary:\n{}",
                branch_label,
                model_id,
                compaction.messages_before,
                compaction.messages_after,
                compaction.tokens_before,
                compaction.tokens_after,
                files_read,
                files_modified,
                compaction.summary
            )
        }
        None => format!(
            "No compaction was necessary for branch {} on model {}.",
            branch_label, model_id
        ),
    }
}

pub(crate) fn render_spawn_output(response: &SpawnSessionResponse) -> String {
    let model = response
        .child_session
        .model_id
        .as_deref()
        .unwrap_or("default");
    let objective = response
        .child_session
        .objective
        .as_deref()
        .unwrap_or("none");
    [
        format!(
            "Spawned child session {}",
            short_id_string(&response.child_session.session_id.to_string())
        ),
        format!(
            "Parent: {} / {} / {}",
            short_id_string(&response.parent_session_id.to_string()),
            short_id_string(&response.parent_branch_id.to_string()),
            response
                .parent_turn_id
                .map(|turn_id| short_id_string(&turn_id.to_string()))
                .unwrap_or_else(|| "no-turn".to_owned())
        ),
        format!(
            "Child branch: {}",
            short_id_string(&response.child_branch.branch_id.to_string())
        ),
        format!(
            "Connection: {}  Model: {}",
            response.child_session.connection_id, model
        ),
        format!("Objective: {objective}"),
        format!(
            "Use /resume {} to open it.",
            short_id_string(&response.child_session.session_id.to_string())
        ),
    ]
    .join("\n")
}

fn preview_export_content(content: &str, limit: usize) -> String {
    let mut chars = content.chars();
    let preview = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}\n... [truncated]")
    } else if preview.is_empty() {
        "<empty>".to_owned()
    } else {
        preview
    }
}

fn format_cost_or_na(cost: f64) -> String {
    if cost > 0.0 {
        format!("${cost:.6}")
    } else {
        "na".to_owned()
    }
}

pub(crate) fn render_queue_output(inspection: &SessionQueueInspection) -> String {
    let mut lines = vec![format!(
        "Queue session={} runtime={} pending_approvals={} queued_messages={} cancel_requested={} pending_steer={}",
        short_id_string(&inspection.session_id.to_string()),
        render_runtime_state(inspection.runtime_state),
        inspection.pending_approvals.len(),
        inspection.queued_messages.len(),
        inspection.cancel_requested,
        inspection.pending_steer_count,
    )];

    if inspection.pending_approvals.is_empty() {
        lines.push("Pending approvals: none".to_owned());
    } else {
        lines.push("Pending approvals:".to_owned());
        lines.extend(inspection.pending_approvals.iter().map(|approval| {
            format!(
                "- {} {} requested_at={}",
                approval.tool_name,
                short_id_string(&approval.call_id.to_string()),
                approval.requested_at
            )
        }));
    }

    if inspection.pending_inputs.is_empty() {
        lines.push("Pending inputs: none".to_owned());
    } else {
        lines.push("Pending inputs:".to_owned());
        lines.extend(inspection.pending_inputs.iter().map(|input| {
            format!(
                "- ask {} requested_at={} prompt={}",
                short_id_string(&input.call_id.to_string()),
                input.requested_at,
                truncate_path(&input.prompt, 100)
            )
        }));
    }

    if inspection.queued_messages.is_empty() {
        lines.push("Session queue: empty".to_owned());
    } else {
        lines.push("Session queue:".to_owned());
        lines.extend(
            inspection
                .queued_messages
                .iter()
                .enumerate()
                .map(|(index, queued)| {
                    format!(
                        "{}. branch={} at={} {}",
                        index + 1,
                        short_id_string(&queued.branch_id.to_string()),
                        queued.enqueued_at,
                        truncate_path(&render_message_preview(&queued.message), 100)
                    )
                }),
        );
    }

    lines.join("\n")
}

pub(crate) fn render_queue_clear_output(response: &SessionQueueClearResponse) -> String {
    let mut lines = vec![format!(
        "Queue clear session={} cleared_session_messages={}",
        short_id_string(&response.session_id.to_string()),
        response.cleared_messages.len(),
    )];

    if response.cleared_messages.is_empty() {
        lines.push("Cleared session queue: empty".to_owned());
    } else {
        lines.push("Cleared session queue:".to_owned());
        lines.extend(
            response
                .cleared_messages
                .iter()
                .enumerate()
                .map(|(index, queued)| {
                    format!(
                        "{}. branch={} at={} {}",
                        index + 1,
                        short_id_string(&queued.branch_id.to_string()),
                        queued.enqueued_at,
                        truncate_path(&render_message_preview(&queued.message), 100)
                    )
                }),
        );
    }

    lines.join("\n")
}

pub(crate) fn render_lineage_output(inspection: &SessionLineageInspection) -> String {
    let mut lines = vec![format!(
        "Lineage focus={} root={} sessions={}",
        short_id_string(&inspection.focus_session_id.to_string()),
        short_id_string(&inspection.root_session_id.to_string()),
        inspection.nodes.len()
    )];
    lines.extend(inspection.nodes.iter().map(|node| {
        let indent = "  ".repeat(node.depth as usize);
        let marker = if node.is_focus { "*" } else { "-" };
        let label = node
            .session
            .display_name
            .clone()
            .unwrap_or_else(|| short_id_string(&node.session.session_id.to_string()));
        format!(
            "{}{} {} status={} conn={} model={} project={}",
            indent,
            marker,
            label,
            render_session_status(&node.session.status),
            node.session.connection_id,
            node.session.model_id.as_deref().unwrap_or("default"),
            truncate_path(node.session.project_root.as_str(), 72)
        )
    }));
    lines.join("\n")
}

pub(crate) fn render_workflow_output(inspection: &SessionWorkflowInspection) -> String {
    let mut lines = vec![
        format!(
            "Workflow focus={} root={} sessions={}",
            short_id_string(&inspection.focus_session_id.to_string()),
            short_id_string(&inspection.root_session_id.to_string()),
            inspection.node_count,
        ),
        format!(
            "Runtime counts: {}",
            render_workflow_runtime_counts(&inspection.runtime_counts)
        ),
        format!(
            "Status counts: {}",
            render_workflow_status_counts(&inspection.status_counts)
        ),
    ];
    lines.extend(inspection.nodes.iter().map(render_workflow_node));
    lines.join("\n")
}

fn render_workflow_runtime_counts(counts: &WorkflowRuntimeCounts) -> String {
    format!(
        "idle={} working={} waiting_on_approval={} cancel_requested={}",
        counts.idle, counts.working, counts.waiting_on_approval, counts.cancel_requested
    )
}

fn render_workflow_status_counts(counts: &WorkflowStatusCounts) -> String {
    format!(
        "active={} completed={} failed={} abandoned={}",
        counts.active, counts.completed, counts.failed, counts.abandoned
    )
}

fn render_workflow_node(node: &WorkflowSessionNode) -> String {
    let indent = "  ".repeat(node.depth as usize);
    let marker = if node.is_focus { "*" } else { "-" };
    let label = node.session.display_name.as_deref().unwrap_or("unnamed");
    let active_branch = node
        .active_branch_id
        .as_ref()
        .map(|branch_id| short_id_string(&branch_id.to_string()))
        .unwrap_or_else(|| "none".to_owned());
    let origin_branch = node
        .session
        .parent_branch_id
        .as_ref()
        .map(|branch_id| short_id_string(&branch_id.to_string()))
        .unwrap_or_else(|| "root".to_owned());
    let origin_turn = node
        .session
        .parent_turn_id
        .as_ref()
        .map(|turn_id| short_id_string(&turn_id.to_string()))
        .unwrap_or_else(|| "n/a".to_owned());
    let last_seq = node
        .last_seq_id
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_owned());
    let objective = node.session.objective.as_deref().unwrap_or("none");
    let project = truncate_path(node.session.project_root.as_str(), 48);

    format!(
        "{indent}{marker} {} session={} status={} runtime={} conn={} model={} branch={} turns={} messages={} tools={} pending_approvals={} children={} last_seq={} origin_branch={} origin_turn={} objective={} project={}",
        label,
        short_id_string(&node.session.session_id.to_string()),
        render_session_status(&node.session.status),
        render_runtime_state(node.runtime_state),
        node.session.connection_id,
        node.session.model_id.as_deref().unwrap_or("default"),
        active_branch,
        node.turn_count,
        node.message_count,
        node.tool_call_count,
        node.pending_approval_count,
        node.child_session_count,
        last_seq,
        origin_branch,
        origin_turn,
        truncate_path(objective, 64),
        project,
    )
}

pub(crate) fn render_branches_output(
    active_branch_id: Option<&bt_core::BranchId>,
    branches: &[BranchInspection],
) -> String {
    if branches.is_empty() {
        return "No branches recorded yet.".to_owned();
    }

    let mut lines = vec![format!(
        "Branches {} active={}",
        branches.len(),
        active_branch_id
            .map(|branch_id| short_id_string(&branch_id.to_string()))
            .unwrap_or_else(|| "none".to_owned())
    )];
    lines.extend(branches.iter().map(render_branch_entry));
    lines.join("\n")
}

fn render_branch_entry(branch: &BranchInspection) -> String {
    let marker = if branch.is_active { "*" } else { "-" };
    let parent = branch
        .branch
        .parent_branch_id
        .as_ref()
        .map(|branch_id| short_id_string(&branch_id.to_string()))
        .unwrap_or_else(|| "root".to_owned());
    let latest_turn = branch
        .latest_turn_id
        .as_ref()
        .map(|turn_id| short_id_string(&turn_id.to_string()))
        .unwrap_or_else(|| "none".to_owned());
    let latest_seq = branch
        .latest_event_seq
        .map(|seq| seq.to_string())
        .unwrap_or_else(|| "none".to_owned());
    let preview = branch
        .latest_message_preview
        .as_deref()
        .unwrap_or("no messages");
    let summary = branch.branch.summary.as_deref().unwrap_or("no summary");

    format!(
        "{marker} {} depth={} parent={} turns={} messages={} local={} latest_turn={} latest_seq={} summary={} preview={}",
        short_id_string(&branch.branch.branch_id.to_string()),
        branch.depth,
        parent,
        branch.turn_count,
        branch.total_message_count,
        branch.local_message_count,
        latest_turn,
        latest_seq,
        truncate_detail(summary),
        truncate_path(preview, 64),
    )
}

pub(crate) fn render_execution_output(
    execution: &SessionExecutionInspection,
    limit: usize,
) -> String {
    let mut lines = vec![
        format!(
            "Execution session={} events={} event_spans={}",
            short_id_string(&execution.session_id.to_string()),
            execution.total_events,
            render_trace_event_counts(&execution.event_counts),
        ),
        format!(
            "Related sessions: {}",
            if execution.related_sessions.is_empty() {
                "none".to_owned()
            } else {
                execution
                    .related_sessions
                    .iter()
                    .map(|session| {
                        format!(
                            "{}:{}",
                            short_id_string(&session.session_id.to_string()),
                            session
                                .display_name
                                .clone()
                                .unwrap_or_else(|| "session".to_owned())
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ),
    ];

    let mut turn_lines = execution
        .turns
        .iter()
        .map(render_trace_turn_entry)
        .collect::<Vec<_>>();
    if turn_lines.len() > limit {
        turn_lines = turn_lines.split_off(turn_lines.len() - limit);
    }

    if turn_lines.is_empty() {
        lines.push("No executed turns recorded yet.".to_owned());
    } else {
        lines.extend(turn_lines);
    }

    lines.join("\n")
}

pub(crate) fn render_tree_output(tree: &SessionTreeInspection) -> String {
    let mut lines = vec![format!(
        "Session {} branches={} active={}",
        short_id_string(&tree.session.session_id.to_string()),
        tree.branches.len(),
        tree.active_branch_id
            .as_ref()
            .map(|branch_id| short_id_string(&branch_id.to_string()))
            .unwrap_or_else(|| "none".to_owned()),
    )];
    lines.push("Branches:".to_owned());
    lines.extend(tree.branches.iter().map(render_tree_branch_entry));
    if tree.related_sessions.is_empty() {
        lines.push("Related sessions: none".to_owned());
    } else {
        lines.push("Related sessions:".to_owned());
        lines.extend(
            tree.related_sessions
                .iter()
                .map(render_related_session_summary),
        );
    }
    lines.join("\n")
}

fn render_tree_branch_entry(branch: &BranchInspection) -> String {
    let indent = "  ".repeat(branch.depth as usize);
    let marker = if branch.is_active { "*" } else { "-" };
    let summary = branch.branch.summary.as_deref().unwrap_or("no summary");
    let preview = branch
        .latest_message_preview
        .as_deref()
        .unwrap_or("no messages");

    format!(
        "{indent}{marker} {} turns={} messages={} local={} summary={} preview={}",
        short_id_string(&branch.branch.branch_id.to_string()),
        branch.turn_count,
        branch.total_message_count,
        branch.local_message_count,
        truncate_detail(summary),
        truncate_path(preview, 56),
    )
}

fn render_trace_turn_entry(turn: &TraceTurnInspection) -> String {
    let usage = turn
        .usage
        .as_ref()
        .map(|usage| format!("tokens={}", usage.total_tokens))
        .unwrap_or_else(|| "tokens=na".to_owned());
    let cost = turn
        .cost
        .as_ref()
        .map(|cost| format!("cost=${:.6}", cost.total_usd))
        .unwrap_or_else(|| "cost=na".to_owned());
    let latency = turn
        .latency_ms
        .map(|latency| format!("latency={}ms", latency))
        .unwrap_or_else(|| "latency=na".to_owned());
    let source = turn
        .source
        .as_ref()
        .map(render_turn_start_source)
        .unwrap_or("unknown");
    let resumed_from = turn
        .resumed_from_call_id
        .as_ref()
        .map(|call_id| short_id_string(&call_id.to_string()))
        .unwrap_or_else(|| "na".to_owned());
    let tools = if turn.tool_calls.is_empty() {
        "tools=none".to_owned()
    } else {
        format!(
            "tools={}",
            turn.tool_calls
                .iter()
                .map(|tool| {
                    format!(
                        "{}:{}:{}",
                        tool.tool_name,
                        tool.approval_status.as_deref().unwrap_or("na"),
                        tool.execution_status.as_deref().unwrap_or("na"),
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    let context = render_context_manifest_summary(turn);

    format!(
        "{} branch={} {} {} source={} resumed_from={} status={} finish={} llm_calls={} approvals={} resumed_after_approval={} raw_chunks={} {} {} {} {} event_spans={} {}",
        short_id_string(&turn.turn_id.to_string()),
        short_id_string(&turn.branch_id.to_string()),
        turn.provider,
        turn.model,
        source,
        resumed_from,
        turn.status.as_deref().unwrap_or("in_progress"),
        turn.finish_reason.as_deref().unwrap_or("none"),
        turn.llm_call_count,
        turn.approval_pause_count,
        if turn.resumed_after_approval {
            "yes"
        } else {
            "no"
        },
        turn.raw_chunk_count,
        latency,
        usage,
        cost,
        context,
        render_trace_event_counts(&turn.event_counts),
        tools,
    )
}

fn render_context_manifest_summary(turn: &TraceTurnInspection) -> String {
    let Some(manifest) = turn.context_manifests.last() else {
        return "contexts=none".to_owned();
    };
    let boundary = manifest
        .context_boundary_seq_id
        .map(|seq_id| seq_id.to_string())
        .unwrap_or_else(|| "none".to_owned());
    format!(
        "contexts={} context_boundary={} context_messages={} context_tools={} context_attachments={} context_compacted={}",
        turn.context_manifests.len(),
        boundary,
        manifest.messages.len(),
        manifest.tools.len(),
        manifest.attachments.len(),
        if manifest.compacted { "yes" } else { "no" },
    )
}

pub(crate) fn render_turn_start_source(source: &bt_core::TurnStartSource) -> &'static str {
    match source {
        bt_core::TurnStartSource::UserMessage => "user_message",
        bt_core::TurnStartSource::ApprovalResume => "approval_resume",
        bt_core::TurnStartSource::InputResume => "input_resume",
        bt_core::TurnStartSource::SteerFollowUp => "steer_follow_up",
        bt_core::TurnStartSource::QueuedFollowUp => "queued_follow_up",
        bt_core::TurnStartSource::RelatedSessionMessage => "related_session_message",
    }
}

fn render_trace_event_counts(counts: &TraceEventCounts) -> String {
    format!(
        "session={} agent={} llm={} tool={} chain={}",
        counts.session, counts.agent, counts.llm, counts.tool, counts.chain
    )
}

fn render_turn_history_entry(turn: &TurnInspection) -> String {
    let tools = if turn.tool_calls.is_empty() {
        "tools=none".to_owned()
    } else {
        let tools = turn
            .tool_calls
            .iter()
            .map(|tool| {
                format!(
                    "{}:{}:{}",
                    tool.tool_name,
                    tool.approval_status.as_deref().unwrap_or("na"),
                    tool.execution_status.as_deref().unwrap_or("na"),
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("tools={tools}")
    };

    format!(
        "{} {} {} status={} finish={} events={} approvals_pending={} raw_chunks={} {}",
        short_id_string(&turn.turn_id.to_string()),
        turn.provider,
        turn.model,
        turn.status.as_deref().unwrap_or("in_progress"),
        turn.finish_reason.as_deref().unwrap_or("none"),
        turn.event_count,
        turn.pending_approval_count,
        turn.raw_chunk_count,
        tools,
    )
}

pub(crate) fn render_session_output(inspection: &SessionInspection) -> String {
    let active_branch = inspection
        .active_branch
        .as_ref()
        .map(|branch| short_id_string(&branch.branch_id.to_string()))
        .unwrap_or_else(|| "none".to_owned());
    let lineage = if inspection.related_sessions.is_empty() {
        "Related sessions: none".to_owned()
    } else {
        let details = inspection
            .related_sessions
            .iter()
            .map(render_related_session_summary)
            .collect::<Vec<_>>()
            .join("\n");
        format!("Related sessions:\n{details}")
    };

    format!(
        "Session {}\nStatus: {:?}  Runtime: {}\nProject: {}\nConnection: {}  Model: {}\nActive branch: {}  Total branches: {}\nTurns: {}  Messages: {}  Tool calls: {}\nApprovals: {} total, {} pending\nRaw chunks: {}  Last seq: {}\nControls: cancel_requested={}  pending_steer={}\n{}",
        short_id_string(&inspection.session.session_id.to_string()),
        inspection.session.status,
        render_runtime_state(inspection.runtime_state),
        truncate_path(inspection.session.project_root.as_str(), 96),
        inspection.session.connection_id,
        inspection.session.model_id.as_deref().unwrap_or("default"),
        active_branch,
        inspection.branches.len(),
        inspection.turn_count,
        inspection.message_count,
        inspection.tool_call_count,
        inspection.approval_count,
        inspection.pending_approval_count,
        inspection.raw_chunk_count,
        inspection
            .last_seq_id
            .map(|seq| seq.to_string())
            .unwrap_or_else(|| "none".to_owned()),
        inspection.cancel_requested,
        inspection.pending_steer_count,
        lineage,
    )
}

fn render_runtime_state(state: SessionRuntimeState) -> &'static str {
    match state {
        SessionRuntimeState::Idle => "idle",
        SessionRuntimeState::Working => "working",
        SessionRuntimeState::WaitingOnInput => "waiting_on_input",
        SessionRuntimeState::WaitingOnApproval => "waiting_on_approval",
        SessionRuntimeState::CancelRequested => "cancel_requested",
    }
}

fn render_session_status(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Active => "active",
        SessionStatus::Completed => "completed",
        SessionStatus::Failed => "failed",
        SessionStatus::Abandoned => "abandoned",
    }
}

fn render_related_session_summary(summary: &RelatedSessionSummary) -> String {
    let relation = match summary.relation {
        bt_core::SessionRelationKind::Parent => "parent",
        bt_core::SessionRelationKind::Child => "child",
    };
    let display = summary.display_name.as_deref().unwrap_or("unnamed");
    let origin_branch = summary
        .origin_branch_id
        .as_ref()
        .map(|value| short_id_string(&value.to_string()))
        .unwrap_or_else(|| "n/a".to_owned());
    let origin_turn = summary
        .origin_turn_id
        .as_ref()
        .map(|value| short_id_string(&value.to_string()))
        .unwrap_or_else(|| "n/a".to_owned());

    format!(
        "- {} {} [{}] conn={} model={} origin_branch={} origin_turn={}",
        relation,
        short_id_string(&summary.session_id.to_string()),
        display,
        summary.connection_id,
        summary.model_id.as_deref().unwrap_or("default"),
        origin_branch,
        origin_turn,
    )
}
