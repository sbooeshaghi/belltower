use bt_core::{
    BranchId, BranchInspection, BranchRecord, BudgetConfig, CompletionDelta, ConnectionAuthState,
    ConnectionDescriptor, ConnectionId, ConnectionModelInventory, ConnectionModelOption,
    ConnectionModelSource, ConnectionReadinessInspection, ConnectionReadinessState,
    ConnectionSupportState, ContextManifest, ContextMessageRef, ContextSystemPromptRef,
    ContextToolRef, CostBreakdown, CredentialKind, ErrorClass, EventEnvelope, EventId,
    EventPayload, HardwareProfile, InstructionDocument, McpServerDescriptor, McpServerStatus,
    McpToolDescriptor, McpTransportKind, Message, MessageId, MessagePart, ModelBackendDescriptor,
    ModelBackendKind, ModelBackendStatus, ModelRecommendation, ModelRecommendationsReport,
    PendingApprovalInspection, PendingInputInspection, QueuedMessageInspection,
    RelatedSessionSummary, Role, SessionCostSummary, SessionExecutionInspection, SessionId,
    SessionInspection, SessionLineageInspection, SessionLineageNode, SessionQueueInspection,
    SessionRecord, SessionRelationKind, SessionRuntimeState, SessionToolMode,
    SessionTreeInspection, SessionWorkflowInspection, SpanId, StatusInspection, TokenUsage,
    ToolCallId, ToolExecutionMode, ToolRiskClass, TraceEventCounts, TraceTurnInspection, TurnId,
    TurnInspection, TurnInstructionProvenance, TurnStartSource, TurnToolCallSummary,
    WorkflowRuntimeCounts, WorkflowSessionNode, WorkflowStatusCounts, default_settings_revision_id,
};
use bt_protocol::{
    CancelSessionRequest, ConnectionModelsResponse, ConnectionsResponse, CreateSessionRequest,
    CreateSessionResponse, ErrorEnvelope, ExportFormat, HealthResponse, McpInventoryResponse,
    McpServersResponse, McpToolsResponse, ModelBackendsResponse, ModelRecommendationsResponse,
    PushOtlpExportRequest, PushOtlpExportResponse, RawSseEnvelope, RecordOperatorCommandRequest,
    RunShellCommandRequest, SendMessageRequest, ServerCapabilities, ServerInfoResponse,
    SessionEventsResponse, SessionExecutionResponse, SessionExportResponse,
    SessionInspectionResponse, SessionLineageResponse, SessionQueueClearResponse,
    SessionQueueResponse, SessionTreeResponse, SessionTurnsResponse, SessionWorkflowResponse,
    SpawnSessionRequest, SpawnSessionResponse, StatusInspectionResponse, SteerSessionRequest,
    UpdateSessionBudgetRequest, UpdateSessionRequest,
};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use time::macros::datetime;
use url::Url;
use uuid::Uuid;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct SessionControlFixtures {
    error_envelope: ErrorEnvelope,
    health_response: HealthResponse,
    create_session_request: CreateSessionRequest,
    create_session_response: CreateSessionResponse,
    update_session_request: UpdateSessionRequest,
    update_session_budget_request: UpdateSessionBudgetRequest,
    cancel_session_request: CancelSessionRequest,
    steer_session_request: SteerSessionRequest,
    record_operator_command_request: RecordOperatorCommandRequest,
    run_shell_command_request: RunShellCommandRequest,
    send_message_request: SendMessageRequest,
    spawn_session_request: SpawnSessionRequest,
    spawn_session_response: SpawnSessionResponse,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct InspectionFixtures {
    session_inspection_response: SessionInspectionResponse,
    session_events_response: SessionEventsResponse,
    session_turns_response: SessionTurnsResponse,
    session_execution_response: SessionExecutionResponse,
    session_queue_response: SessionQueueResponse,
    session_queue_clear_response: SessionQueueClearResponse,
    session_lineage_response: SessionLineageResponse,
    session_workflow_response: SessionWorkflowResponse,
    session_tree_response: SessionTreeResponse,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct OperatorFixtures {
    connections_response: ConnectionsResponse,
    server_info_response: ServerInfoResponse,
    status_inspection_response: StatusInspectionResponse,
    mcp_inventory_response: McpInventoryResponse,
    mcp_servers_response: McpServersResponse,
    mcp_tools_response: McpToolsResponse,
    model_backends_response: ModelBackendsResponse,
    connection_models_response: ConnectionModelsResponse,
    model_recommendations_response: ModelRecommendationsResponse,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct ExportFixtures {
    session_export_response: SessionExportResponse,
    push_otlp_export_request: PushOtlpExportRequest,
    push_otlp_export_response: PushOtlpExportResponse,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct StreamFixtures {
    raw_sse_envelope: RawSseEnvelope,
}

#[test]
fn session_control_fixtures_round_trip() {
    assert_fixture("session_control.json", &session_control_fixtures());
}

#[test]
fn inspection_fixtures_round_trip() {
    assert_fixture("inspection.json", &inspection_fixtures());
}

#[test]
fn operator_surface_fixtures_round_trip() {
    assert_fixture("operator_surfaces.json", &operator_fixtures());
}

#[test]
fn export_fixtures_round_trip() {
    assert_fixture("export.json", &export_fixtures());
}

#[test]
fn stream_fixtures_round_trip() {
    assert_fixture("stream.json", &stream_fixtures());
}

fn assert_fixture<T>(name: &str, expected: &T)
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let path = fixture_path(name);
    let actual = format!(
        "{}\n",
        serde_json::to_string_pretty(expected).expect("serialize fixture")
    );

    if std::env::var_os("BLESS_PROTOCOL_FIXTURES").is_some() {
        fs::create_dir_all(path.parent().expect("fixture dir")).expect("create fixture dir");
        fs::write(&path, &actual).expect("write fixture");
    }

    let raw = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read fixture {}: {error}", path.display()));
    let parsed: T = serde_json::from_str(&raw).unwrap_or_else(|error| {
        panic!("failed to deserialize fixture {}: {error}", path.display())
    });

    assert_eq!(
        parsed,
        *expected,
        "fixture {} should deserialize",
        path.display()
    );
    assert_eq!(raw, actual, "fixture {} drifted", path.display());
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn session_control_fixtures() -> SessionControlFixtures {
    SessionControlFixtures {
        error_envelope: ErrorEnvelope {
            class: ErrorClass::Provider,
            code: "provider_error".to_owned(),
            message: "provider stream failed".to_owned(),
            retryable: true,
            details: Some(json!({
                "provider": "openai",
                "request_id": "req_123"
            })),
        },
        health_response: HealthResponse {
            status: "ok".to_owned(),
            protocol_version: bt_protocol::PROTOCOL_VERSION.to_owned(),
        },
        create_session_request: CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/belltower/project".to_owned(),
            connection_id: connection_id("openai"),
            model_id: Some("gpt-5.1".to_owned()),
            tool_mode: Some(SessionToolMode::Extended),
            display_name: Some("Protocol fixture session".to_owned()),
            objective: Some("Verify protocol fixtures".to_owned()),
            budget: Some(sample_budget_config()),
        },
        create_session_response: CreateSessionResponse {
            session: session_record(),
            branch: branch_record(),
        },
        update_session_request: UpdateSessionRequest {
            branch_id: branch_id(),
            connection_id: Some(connection_id("local")),
            model_id: Some("qwen2.5-coder:7b".to_owned()),
            tool_mode: Some(SessionToolMode::Standard),
            reset_model_to_default: false,
        },
        update_session_budget_request: UpdateSessionBudgetRequest {
            branch_id: branch_id(),
            budget: sample_budget_config(),
        },
        cancel_session_request: CancelSessionRequest {
            branch_id: branch_id(),
            reason: Some("operator requested cancellation".to_owned()),
        },
        steer_session_request: SteerSessionRequest {
            branch_id: branch_id(),
            message: "Focus on the event-log invariant.".to_owned(),
        },
        record_operator_command_request: RecordOperatorCommandRequest {
            branch_id: branch_id(),
            command_type: "slash_command".to_owned(),
            raw_input: "/status".to_owned(),
            output: "runtime ready".to_owned(),
            success: true,
        },
        run_shell_command_request: RunShellCommandRequest {
            branch_id: branch_id(),
            raw_input: "!git status --short".to_owned(),
            command: "git status --short".to_owned(),
            timeout_seconds: Some(30),
        },
        send_message_request: SendMessageRequest {
            branch_id: branch_id(),
            message: user_message("Please inspect the protocol surface."),
        },
        spawn_session_request: SpawnSessionRequest {
            dispatch: None,
            parent_branch_id: branch_id(),
            parent_turn_id: Some(turn_id()),
            objective: "Review MCP state transitions".to_owned(),
            display_name: Some("MCP child".to_owned()),
            connection_id: Some(connection_id("anthropic")),
            model_id: Some("claude-3-7-sonnet-20250219".to_owned()),
        },
        spawn_session_response: SpawnSessionResponse {
            parent_session_id: session_id(),
            parent_branch_id: branch_id(),
            parent_turn_id: Some(turn_id()),
            child_session: child_session_record(),
            child_branch: child_branch_record(),
        },
    }
}

fn sample_budget_config() -> BudgetConfig {
    BudgetConfig {
        max_wall_clock_seconds: Some(300),
        max_tokens: Some(120_000),
        max_turns: Some(12),
        max_cost_usd: Some(8.5),
    }
}

fn inspection_fixtures() -> InspectionFixtures {
    InspectionFixtures {
        session_inspection_response: SessionInspectionResponse {
            inspection: SessionInspection {
                session: session_record(),
                active_branch: Some(branch_record()),
                branches: vec![branch_record(), child_branch_record()],
                runtime_state: SessionRuntimeState::Working,
                turn_count: 3,
                message_count: 5,
                tool_call_count: 2,
                approval_count: 1,
                pending_approval_count: 1,
                pending_input_count: 1,
                raw_chunk_count: 4,
                last_seq_id: Some(42),
                cancel_requested: false,
                pending_steer_count: 1,
                related_sessions: vec![related_child_summary()],
                cost_summary: Some(SessionCostSummary {
                    prompt_tokens: 100,
                    completion_tokens: 40,
                    total_tokens: 140,
                    total_cost_usd: 0.0042,
                    unpriced_completion_count: 1,
                    updated_at: ts(12),
                }),
                budget: None,
            },
        },
        session_events_response: SessionEventsResponse {
            session_id: session_id(),
            events: vec![instruction_provenance_event(), event_envelope()],
            last_seq_id: Some(42),
        },
        session_turns_response: SessionTurnsResponse {
            session_id: session_id(),
            turns: vec![turn_inspection()],
        },
        session_execution_response: SessionExecutionResponse {
            inspection: SessionExecutionInspection {
                session_id: session_id(),
                total_events: 7,
                event_counts: TraceEventCounts {
                    session: 2,
                    agent: 1,
                    llm: 2,
                    tool: 1,
                    chain: 1,
                },
                related_sessions: vec![related_child_summary()],
                turns: vec![trace_turn_inspection()],
            },
        },
        session_queue_response: SessionQueueResponse {
            inspection: SessionQueueInspection {
                session_id: session_id(),
                runtime_state: SessionRuntimeState::WaitingOnApproval,
                cancel_requested: false,
                pending_steer_count: 1,
                queued_messages: vec![QueuedMessageInspection {
                    branch_id: branch_id(),
                    enqueued_at: ts(8),
                    message: user_message("Queued follow-up message."),
                    settings_revision_id: default_settings_revision_id(),
                }],
                pending_approvals: vec![PendingApprovalInspection {
                    call_id: ToolCallId::new("call-approval-1"),
                    tool_name: "shell".to_owned(),
                    branch_id: branch_id(),
                    turn_id: turn_id(),
                    settings_revision_id: default_settings_revision_id(),
                    requested_at: ts(9),
                    request_snapshot: None,
                }],
                pending_inputs: vec![PendingInputInspection {
                    call_id: ToolCallId::new("call-input-1"),
                    prompt: "Choose the deployment target".to_owned(),
                    choices: vec!["staging".to_owned(), "prod".to_owned()],
                    branch_id: branch_id(),
                    turn_id: turn_id(),
                    settings_revision_id: default_settings_revision_id(),
                    requested_at: ts(10),
                }],
            },
        },
        session_queue_clear_response: SessionQueueClearResponse {
            session_id: session_id(),
            cleared_messages: vec![QueuedMessageInspection {
                branch_id: branch_id(),
                enqueued_at: ts(11),
                message: user_message("Cleared queued follow-up message."),
                settings_revision_id: default_settings_revision_id(),
            }],
        },
        session_lineage_response: SessionLineageResponse {
            inspection: SessionLineageInspection {
                focus_session_id: session_id(),
                root_session_id: session_id(),
                nodes: vec![
                    SessionLineageNode {
                        session: session_record(),
                        depth: 0,
                        is_focus: true,
                    },
                    SessionLineageNode {
                        session: child_session_record(),
                        depth: 1,
                        is_focus: false,
                    },
                ],
            },
        },
        session_workflow_response: SessionWorkflowResponse {
            inspection: SessionWorkflowInspection {
                focus_session_id: session_id(),
                root_session_id: session_id(),
                node_count: 2,
                runtime_counts: WorkflowRuntimeCounts {
                    idle: 1,
                    working: 1,
                    waiting_on_input: 0,
                    waiting_on_approval: 0,
                    cancel_requested: 0,
                },
                status_counts: WorkflowStatusCounts {
                    active: 2,
                    completed: 0,
                    failed: 0,
                    abandoned: 0,
                },
                nodes: vec![
                    WorkflowSessionNode {
                        session: session_record(),
                        depth: 0,
                        is_focus: true,
                        runtime_state: SessionRuntimeState::Working,
                        active_branch_id: Some(branch_id()),
                        turn_count: 3,
                        message_count: 5,
                        tool_call_count: 2,
                        pending_approval_count: 1,
                        child_session_count: 1,
                        last_seq_id: Some(42),
                    },
                    WorkflowSessionNode {
                        session: child_session_record(),
                        depth: 1,
                        is_focus: false,
                        runtime_state: SessionRuntimeState::Idle,
                        active_branch_id: Some(child_branch_id()),
                        turn_count: 1,
                        message_count: 1,
                        tool_call_count: 0,
                        pending_approval_count: 0,
                        child_session_count: 0,
                        last_seq_id: Some(52),
                    },
                ],
            },
        },
        session_tree_response: SessionTreeResponse {
            inspection: SessionTreeInspection {
                session: session_record(),
                active_branch_id: Some(branch_id()),
                branches: vec![branch_inspection()],
                related_sessions: vec![related_child_summary()],
            },
        },
    }
}

fn operator_fixtures() -> OperatorFixtures {
    OperatorFixtures {
        connections_response: ConnectionsResponse {
            connections: vec![ConnectionDescriptor {
                id: connection_id("openai"),
                provider: "openai".to_owned(),
                base_url: url("https://api.openai.com/v1/"),
                default_model: "gpt-5.1".to_owned(),
                auth_methods: Vec::new(),
                auth_sources: vec!["auth_store".to_owned(), "OPENAI_API_KEY".to_owned()],
                model_fallbacks: vec!["gpt-5-mini".to_owned(), "gpt-5-nano".to_owned()],
                discoverable_model_selectors: Vec::new(),
            }],
        },
        server_info_response: ServerInfoResponse {
            server_version: "0.1.0".to_owned(),
            protocol_version: "belltower.v1".to_owned(),
            supported_protocol_versions: vec!["belltower.v1".to_owned()],
            capabilities: ServerCapabilities {
                approvals: true,
                pending_input: true,
                session_queue: true,
                workflow_inspection: true,
                lineage_inspection: true,
                raw_chunk_paging: true,
                exports: true,
                mcp_inventory: true,
                mcp_reload: true,
                spawn_session: true,
            },
        },
        status_inspection_response: StatusInspectionResponse {
            inspection: StatusInspection {
                auth_storage: "auto (ephemeral env vars, keychain default write when available, auth store (/tmp/belltower/auth.json) fallback)".to_owned(),
                default_connection: connection_id("openai"),
                connections: vec![ConnectionReadinessInspection {
                    connection_id: connection_id("openai"),
                    provider: "openai".to_owned(),
                    default_model: "gpt-5.1".to_owned(),
                    auth_methods: Vec::new(),
                    auth_state: ConnectionAuthState::Configured,
                    auth_kind: Some(CredentialKind::ApiKey),
                    auth_source: Some("keychain (service belltower, account openai)".to_owned()),
                    auth_tried_sources: vec![
                        "ephemeral environment".to_owned(),
                        "keychain (service belltower)".to_owned(),
                    ],
                    support_state: ConnectionSupportState::RuntimeSupported,
                    readiness_state: ConnectionReadinessState::Ready,
                    probe_ready: true,
                    probe_status: "ok".to_owned(),
                }],
            },
        },
        mcp_servers_response: McpServersResponse {
            servers: vec![McpServerDescriptor {
                name: "docs".to_owned(),
                transport: McpTransportKind::StreamableHttp,
                enabled: true,
                status: McpServerStatus::Ready,
            }],
        },
        mcp_inventory_response: McpInventoryResponse {
            servers: vec![McpServerDescriptor {
                name: "docs".to_owned(),
                transport: McpTransportKind::StreamableHttp,
                enabled: true,
                status: McpServerStatus::Ready,
            }],
            tools: vec![McpToolDescriptor {
                server_name: "docs".to_owned(),
                tool_name: "search_docs".to_owned(),
                qualified_name: "docs.search_docs".to_owned(),
                description: "Search the documentation corpus".to_owned(),
                parameters_schema: json!({
                    "type": "object",
                    "required": ["query"],
                    "properties": {
                        "query": {"type": "string"}
                    }
                }),
            }],
        },
        mcp_tools_response: McpToolsResponse {
            tools: vec![McpToolDescriptor {
                server_name: "docs".to_owned(),
                tool_name: "search_docs".to_owned(),
                qualified_name: "docs.search_docs".to_owned(),
                description: "Search the documentation corpus".to_owned(),
                parameters_schema: json!({
                    "type": "object",
                    "required": ["query"],
                    "properties": {
                        "query": {"type": "string"}
                    }
                }),
            }],
        },
        model_backends_response: ModelBackendsResponse {
            backends: vec![ModelBackendDescriptor {
                kind: ModelBackendKind::Ollama,
                label: "Ollama".to_owned(),
                base_url: url("http://127.0.0.1:11434/"),
                status: ModelBackendStatus::Ready,
                available_models: vec!["qwen2.5-coder:7b".to_owned(), "gpt-oss:20b".to_owned()],
            }],
        },
        connection_models_response: ConnectionModelsResponse {
            connections: vec![ConnectionModelInventory {
                connection_id: connection_id("openai"),
                provider: "openai".to_owned(),
                default_model: "gpt-5.1".to_owned(),
                auth_methods: Vec::new(),
                auth_state: ConnectionAuthState::Configured,
                auth_source: Some("keychain (service belltower, account openai)".to_owned()),
                support_state: ConnectionSupportState::RuntimeSupported,
                readiness_state: ConnectionReadinessState::Ready,
                probe_ready: true,
                probe_status: "ok".to_owned(),
                discovered_source: Some("provider_api".to_owned()),
                models: vec![
                    ConnectionModelOption {
                        model_id: "gpt-5.1".to_owned(),
                        source: ConnectionModelSource::Default,
                    },
                    ConnectionModelOption {
                        model_id: "gpt-5-mini".to_owned(),
                        source: ConnectionModelSource::Fallback,
                    },
                    ConnectionModelOption {
                        model_id: "gpt-5.2".to_owned(),
                        source: ConnectionModelSource::Discovered,
                    },
                ],
            }],
        },
        model_recommendations_response: ModelRecommendationsResponse {
            report: ModelRecommendationsReport {
                hardware: HardwareProfile {
                    total_memory_gb: Some(64),
                },
                recommendations: vec![ModelRecommendation {
                    label: "Laptop".to_owned(),
                    max_memory_gb: 64,
                    models: vec!["qwen2.5-coder:7b".to_owned(), "gpt-oss:20b".to_owned()],
                    fits_hardware: true,
                }],
            },
        },
    }
}

fn export_fixtures() -> ExportFixtures {
    let mut headers = BTreeMap::new();
    headers.insert("x-api-key".to_owned(), "secret".to_owned());
    headers.insert("x-project".to_owned(), "belltower".to_owned());

    ExportFixtures {
        session_export_response: SessionExportResponse {
            session_id: session_id(),
            format: ExportFormat::Jsonl,
            content_type: "application/x-ndjson".to_owned(),
            content: "{\"kind\":\"session\"}\n".to_owned(),
        },
        push_otlp_export_request: PushOtlpExportRequest {
            endpoint: Some("https://phoenix.example.com".to_owned()),
            project_name: Some("belltower-dev".to_owned()),
            api_key: None,
            headers,
        },
        push_otlp_export_response: PushOtlpExportResponse {
            session_id: session_id(),
            request_url: "https://phoenix.example.com/v1/traces".to_owned(),
            content_type: "application/x-protobuf".to_owned(),
            bytes_sent: 2048,
            status_code: 200,
            rejected_spans: Some(0),
            warning: None,
        },
    }
}

fn stream_fixtures() -> StreamFixtures {
    StreamFixtures {
        raw_sse_envelope: RawSseEnvelope {
            id: 42,
            event: "turn.finished".to_owned(),
            protocol_version: bt_protocol::PROTOCOL_VERSION.to_owned(),
            data: json!({
                "seq_id": 42,
                "event_id": event_id().to_string(),
                "status": "completed"
            }),
        },
    }
}

fn session_record() -> SessionRecord {
    SessionRecord {
        session_id: session_id(),
        project_root: Utf8PathBuf::from("/tmp/belltower/project"),
        connection_id: connection_id("openai"),
        model_id: Some("gpt-5.1".to_owned()),
        tool_mode: SessionToolMode::Extended,
        settings_revision_id: default_settings_revision_id(),
        created_at: ts(0),
        updated_at: ts(12),
        status: bt_core::SessionStatus::Active,
        display_name: Some("Protocol fixture session".to_owned()),
        objective: Some("Verify protocol fixtures".to_owned()),
        parent_session_id: None,
        parent_branch_id: None,
        parent_turn_id: None,
    }
}

fn child_session_record() -> SessionRecord {
    SessionRecord {
        session_id: child_session_id(),
        project_root: Utf8PathBuf::from("/tmp/belltower/project"),
        connection_id: connection_id("anthropic"),
        model_id: Some("claude-3-7-sonnet-20250219".to_owned()),
        tool_mode: SessionToolMode::Standard,
        settings_revision_id: default_settings_revision_id(),
        created_at: ts(20),
        updated_at: ts(25),
        status: bt_core::SessionStatus::Active,
        display_name: Some("MCP child".to_owned()),
        objective: Some("Review MCP state transitions".to_owned()),
        parent_session_id: Some(session_id()),
        parent_branch_id: Some(branch_id()),
        parent_turn_id: Some(turn_id()),
    }
}

fn branch_record() -> BranchRecord {
    BranchRecord {
        branch_id: branch_id(),
        session_id: session_id(),
        parent_branch_id: None,
        parent_event_id: Some(event_id()),
        head_event_id: Some(event_id()),
        summary: Some("Main branch summary".to_owned()),
        created_at: ts(0),
        is_default: true,
    }
}

fn child_branch_record() -> BranchRecord {
    BranchRecord {
        branch_id: child_branch_id(),
        session_id: child_session_id(),
        parent_branch_id: Some(branch_id()),
        parent_event_id: Some(event_id()),
        head_event_id: Some(child_event_id()),
        summary: Some("Child branch summary".to_owned()),
        created_at: ts(20),
        is_default: true,
    }
}

fn branch_inspection() -> BranchInspection {
    BranchInspection {
        branch: branch_record(),
        depth: 0,
        is_active: true,
        total_message_count: 5,
        local_message_count: 3,
        turn_count: 3,
        latest_turn_id: Some(turn_id()),
        latest_event_seq: Some(42),
        latest_message_preview: Some("Protocol fixtures are green.".to_owned()),
    }
}

fn related_child_summary() -> RelatedSessionSummary {
    RelatedSessionSummary {
        relation: SessionRelationKind::Child,
        session_id: child_session_id(),
        display_name: Some("MCP child".to_owned()),
        objective: Some("Review MCP state transitions".to_owned()),
        status: bt_core::SessionStatus::Active,
        connection_id: connection_id("anthropic"),
        model_id: Some("claude-3-7-sonnet-20250219".to_owned()),
        origin_branch_id: Some(branch_id()),
        origin_turn_id: Some(turn_id()),
        updated_at: ts(25),
    }
}

fn turn_inspection() -> TurnInspection {
    TurnInspection {
        turn_id: turn_id(),
        branch_id: branch_id(),
        provider: "openai".to_owned(),
        model: "gpt-5.1".to_owned(),
        message_count: 3,
        settings_revision_id: default_settings_revision_id(),
        started_at: ts(5),
        finished_at: Some(ts(6)),
        status: Some("completed".to_owned()),
        finish_reason: Some("stop".to_owned()),
        latency_ms: Some(1200),
        event_seq_start: Some(30),
        event_seq_end: Some(42),
        event_count: 12,
        pending_approval_count: 1,
        raw_chunk_count: 4,
        tool_calls: vec![TurnToolCallSummary {
            call_id: ToolCallId::new("call-shell-1"),
            tool_name: "shell".to_owned(),
            approval_status: Some("approved".to_owned()),
            execution_status: Some("completed".to_owned()),
        }],
    }
}

fn trace_turn_inspection() -> TraceTurnInspection {
    TraceTurnInspection {
        turn_id: turn_id(),
        branch_id: branch_id(),
        settings_revision_id: default_settings_revision_id(),
        source: Some(TurnStartSource::ApprovalResume),
        resumed_from_call_id: Some(ToolCallId::new("call-shell-1")),
        provider: "openai".to_owned(),
        model: "gpt-5.1".to_owned(),
        started_at: ts(5),
        finished_at: Some(ts(6)),
        status: Some("completed".to_owned()),
        finish_reason: Some("stop".to_owned()),
        latency_ms: Some(1200),
        event_seq_start: Some(30),
        event_seq_end: Some(42),
        event_count: 12,
        raw_chunk_count: 4,
        llm_call_count: 1,
        approval_pause_count: 1,
        resumed_after_approval: true,
        event_counts: TraceEventCounts {
            session: 2,
            agent: 1,
            llm: 2,
            tool: 1,
            chain: 1,
        },
        usage: Some(TokenUsage {
            prompt_tokens: 120,
            completion_tokens: 64,
            total_tokens: 184,
            cache_read_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: Some(12),
        }),
        cost: Some(CostBreakdown {
            prompt_usd: 0.0012,
            completion_usd: 0.0024,
            total_usd: 0.0036,
            cache_read_usd: None,
            cache_write_usd: None,
            reasoning_usd: Some(0.0002),
        }),
        context_manifests: vec![ContextManifest {
            turn_id: turn_id(),
            branch_id: branch_id(),
            llm_call_ordinal: 1,
            provider: "openai".to_owned(),
            model: "gpt-5.1".to_owned(),
            settings_revision_id: default_settings_revision_id(),
            context_boundary_seq_id: Some(29),
            system_prompt: ContextSystemPromptRef {
                present: true,
                char_count: 128,
            },
            messages: vec![ContextMessageRef {
                message_id: message_id(),
                role: Role::User,
                source_branch_id: Some(branch_id()),
                source_seq_id: Some(28),
                part_count: 1,
                visible_text_chars: 42,
            }],
            tools: vec![ContextToolRef {
                name: "shell".to_owned(),
                risk_class: ToolRiskClass::Moderate,
                is_read_only: false,
                execution_mode: ToolExecutionMode::UserInput,
                should_defer: false,
            }],
            attachments: Vec::new(),
            max_tokens: Some(2048),
            thinking: None,
            compacted: false,
        }],
        tool_calls: vec![TurnToolCallSummary {
            call_id: ToolCallId::new("call-shell-1"),
            tool_name: "shell".to_owned(),
            approval_status: Some("approved".to_owned()),
            execution_status: Some("completed".to_owned()),
        }],
    }
}

fn event_envelope() -> EventEnvelope {
    let mut attributes = BTreeMap::new();
    attributes.insert("session.id".to_owned(), json!(session_id().to_string()));
    attributes.insert("turn.id".to_owned(), json!(turn_id().to_string()));

    EventEnvelope {
        seq_id: Some(42),
        event_id: child_event_id(),
        session_id: session_id(),
        branch_id: branch_id(),
        turn_id: Some(turn_id()),
        span_id: parent_span_id(),
        parent_span_id: Some(span_id()),
        span_kind: bt_core::SpanKind::Llm,
        occurred_at: ts(6),
        payload: EventPayload::CompletionChunk {
            llm_call_ordinal: Some(1),
            deltas: vec![
                CompletionDelta::AppendText {
                    text: "hello".to_owned(),
                },
                CompletionDelta::AppendRefusal {
                    text: None,
                    provider_reason: Some("safety".to_owned()),
                    opaque_metadata: Some(json!({"rule": "example"})),
                },
            ],
            raw_chunk_index: Some(7),
        },
        attributes,
    }
}

fn instruction_provenance_event() -> EventEnvelope {
    let mut attributes = BTreeMap::new();
    attributes.insert("session.id".to_owned(), json!(session_id().to_string()));
    attributes.insert("turn.id".to_owned(), json!(turn_id().to_string()));

    EventEnvelope {
        seq_id: Some(41),
        event_id: event_id(),
        session_id: session_id(),
        branch_id: branch_id(),
        turn_id: Some(turn_id()),
        span_id: span_id(),
        parent_span_id: None,
        span_kind: bt_core::SpanKind::Agent,
        occurred_at: ts(5),
        payload: EventPayload::TurnInstructionProvenanceRecorded {
            provenance: instruction_provenance(),
        },
        attributes,
    }
}

fn instruction_provenance() -> TurnInstructionProvenance {
    TurnInstructionProvenance {
        turn_id: turn_id(),
        provider: "openai".to_owned(),
        model: "gpt-5.1".to_owned(),
        settings_revision_id: default_settings_revision_id(),
        core_prompt: instruction_document(
            "builtin://core-prompt",
            "Belltower Core Prompt",
            "You are Belltower.",
        ),
        provider_overlay: Some(instruction_document(
            "provider://openai",
            "OpenAI Overlay",
            "Prefer concise tool plans.",
        )),
        instructions: vec![
            instruction_document(
                "file:///tmp/belltower/AGENTS.md",
                "AGENTS.md",
                "Follow repository rules.",
            ),
            instruction_document(
                "file:///tmp/belltower/docs/development/developer-guidelines.md",
                "Developer Guidelines",
                "Honor modular seams.",
            ),
        ],
        rendered_system_prompt:
            "You are Belltower.\nPrefer concise tool plans.\nFollow repository rules.\nHonor modular seams."
                .to_owned(),
    }
}

fn instruction_document(source: &str, title: &str, body: &str) -> InstructionDocument {
    InstructionDocument {
        source: source.to_owned(),
        title: title.to_owned(),
        body: body.to_owned(),
    }
}

fn user_message(text: &str) -> Message {
    Message {
        message_id: message_id(),
        role: Role::User,
        parts: vec![MessagePart::Text {
            text: text.to_owned(),
        }],
        created_at: ts(4),
    }
}

fn session_id() -> SessionId {
    uuid("00000000-0000-0000-0000-000000000001").into()
}

fn child_session_id() -> SessionId {
    uuid("00000000-0000-0000-0000-000000000002").into()
}

fn branch_id() -> BranchId {
    uuid("20000000-0000-0000-0000-000000000001").into()
}

fn child_branch_id() -> BranchId {
    uuid("20000000-0000-0000-0000-000000000002").into()
}

fn turn_id() -> TurnId {
    uuid("30000000-0000-0000-0000-000000000001").into()
}

fn event_id() -> EventId {
    uuid("40000000-0000-0000-0000-000000000001").into()
}

fn child_event_id() -> EventId {
    uuid("40000000-0000-0000-0000-000000000002").into()
}

fn span_id() -> SpanId {
    uuid("50000000-0000-0000-0000-000000000001").into()
}

fn parent_span_id() -> SpanId {
    uuid("50000000-0000-0000-0000-000000000002").into()
}

fn message_id() -> MessageId {
    uuid("60000000-0000-0000-0000-000000000001").into()
}

fn connection_id(value: &str) -> ConnectionId {
    ConnectionId::new(value)
}

fn ts(minutes: i64) -> OffsetDateTime {
    datetime!(2026-03-31 12:00:00 UTC) + time::Duration::minutes(minutes)
}

fn url(value: &str) -> Url {
    Url::parse(value).expect("valid url")
}

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).expect("valid uuid")
}
