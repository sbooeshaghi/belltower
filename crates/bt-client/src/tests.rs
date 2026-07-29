use super::{
    BelltowerClient, ClientError, discovered_auth_token_paths, load_auth_token_from_paths,
};
use axum::Json;
use axum::Router;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::routing::get;
use axum::routing::post;
use bt_core::{
    BranchId, BranchRecord, ConnectionAuthState, ConnectionDescriptor, ConnectionId,
    ConnectionModelInventory, ConnectionModelOption, ConnectionModelSource,
    ConnectionReadinessInspection, ConnectionReadinessState, ConnectionSupportState,
    CredentialKind, ErrorClass, McpServerDescriptor, McpServerStatus, McpToolDescriptor,
    McpTransportKind, Message, ModelBackendDescriptor, ModelBackendKind, ModelBackendStatus,
    ModelRecommendation, ModelRecommendationsReport, QueuedMessageInspection, Role, SessionId,
    SessionQueueInspection, SessionRecord, SessionRuntimeState, SessionStatus, SessionToolMode,
    SessionWorkflowInspection, StatusInspection, WorkflowRuntimeCounts, WorkflowSessionNode,
    WorkflowStatusCounts, auth_token_path, default_settings_revision_id,
    server_auth_token_path_for_url,
};
use bt_protocol::{
    ConnectionModelsResponse, ConnectionsResponse, CreateSessionRequest, CreateSessionResponse,
    HealthResponse, McpInventoryResponse, McpServersResponse, McpToolsResponse,
    ModelBackendsResponse, ModelRecommendationsResponse, PROTOCOL_HEADER, PROTOCOL_VERSION,
    RawSseEnvelope, SendMessageOutcome, SendMessageRequest, SendMessageResponse,
    ServerCapabilities, ServerInfoResponse, SessionQueueClearResponse, SessionQueueResponse,
    SessionWorkflowResponse, StatusInspectionResponse, UpdateSessionRequest,
};
use futures_util::StreamExt;
use serde_json::json;
use std::convert::Infallible;
use tempfile::NamedTempFile;
use tokio::net::TcpListener;
use url::Url;

#[tokio::test]
async fn stream_events_parses_sse_envelopes() {
    async fn stream() -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
        let first = serde_json::to_string(&RawSseEnvelope {
            id: 1,
            event: "session.started".to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
            data: json!({"kind": "first"}),
        })
        .expect("serialize");
        let second = serde_json::to_string(&RawSseEnvelope {
            id: 2,
            event: "session.steered".to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
            data: json!({"kind": "second"}),
        })
        .expect("serialize");
        let events = vec![Event::default().data(first), Event::default().data(second)];
        Sse::new(futures_util::stream::iter(
            events.into_iter().map(Ok::<_, Infallible>),
        ))
    }

    let app = Router::new().route("/sessions/{session_id}/events/stream", get(stream));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let token_file = NamedTempFile::new().expect("token file");
    std::fs::write(token_file.path(), "test-token").expect("write token");
    let client = BelltowerClient::try_from(format!("http://{addr}/").as_str())
        .expect("client")
        .with_auth_token_path(token_file.path());

    let mut stream = client
        .stream_events(SessionId::new(), None)
        .await
        .expect("event stream");
    let first = stream
        .next()
        .await
        .expect("first event")
        .expect("first envelope");
    let second = stream
        .next()
        .await
        .expect("second event")
        .expect("second envelope");

    server.abort();

    assert_eq!(first.id, 1);
    assert_eq!(first.event, "session.started");
    assert_eq!(second.id, 2);
    assert_eq!(second.event, "session.steered");
}

#[tokio::test]
async fn send_message_surfaces_api_error_envelopes() {
    async fn fail() -> (StatusCode, axum::Json<serde_json::Value>) {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({
                "class": "provider",
                "code": "internal_error",
                "message": "missing API key for openai",
                "retryable": false
            })),
        )
    }

    let app = Router::new().route("/sessions/{session_id}/message", post(fail));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let token_file = NamedTempFile::new().expect("token file");
    std::fs::write(token_file.path(), "test-token").expect("write token");
    let client = BelltowerClient::try_from(format!("http://{addr}/").as_str())
        .expect("client")
        .with_auth_token_path(token_file.path());

    let error = client
        .send_message(
            SessionId::new(),
            &SendMessageRequest {
                branch_id: bt_core::BranchId::new(),
                message: bt_core::Message::text(bt_core::Role::User, "hello"),
            },
        )
        .await
        .expect_err("api error");

    server.abort();

    match error {
        ClientError::Api {
            status,
            class,
            code,
            message,
            retryable,
        } => {
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(class, ErrorClass::Provider);
            assert_eq!(code.as_deref(), Some("internal_error"));
            assert!(message.contains("missing API key"));
            assert!(!retryable);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn send_message_returns_structured_outcome() {
    let expected_session_id = SessionId::new();
    let expected_branch_id = BranchId::new();
    let app = Router::new().route(
        "/sessions/{session_id}/message",
        post({
            move || async move {
                (
                    StatusCode::ACCEPTED,
                    axum::Json(SendMessageResponse {
                        session_id: expected_session_id,
                        branch_id: expected_branch_id,
                        outcome: SendMessageOutcome::Queued { position: 2 },
                    }),
                )
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let token_file = NamedTempFile::new().expect("token file");
    std::fs::write(token_file.path(), "test-token").expect("write token");
    let client = BelltowerClient::try_from(format!("http://{addr}/").as_str())
        .expect("client")
        .with_auth_token_path(token_file.path());

    let response = client
        .send_message(
            SessionId::new(),
            &SendMessageRequest {
                branch_id: BranchId::new(),
                message: Message::text(Role::User, "hello"),
            },
        )
        .await
        .expect("send message response");

    server.abort();

    assert_eq!(response.session_id, expected_session_id);
    assert_eq!(response.branch_id, expected_branch_id);
    assert_eq!(response.outcome, SendMessageOutcome::Queued { position: 2 });
}

#[tokio::test]
async fn health_uses_protocol_header_without_local_auth_discovery() {
    async fn health(
        headers: axum::http::HeaderMap,
    ) -> std::result::Result<Json<HealthResponse>, StatusCode> {
        let protocol_ok = headers
            .get(PROTOCOL_HEADER)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == PROTOCOL_VERSION);
        let auth_present = headers.get(axum::http::header::AUTHORIZATION).is_some();
        if !protocol_ok || auth_present {
            return Err(StatusCode::BAD_REQUEST);
        }
        Ok(Json(HealthResponse {
            status: "ok".to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
        }))
    }

    let app = Router::new().route("/health", get(health));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let client =
        BelltowerClient::new_discovering_auth(Url::parse(&format!("http://{addr}/")).expect("url"))
            .expect("client");
    let response = client.health().await.expect("health");

    server.abort();

    assert_eq!(response.status, "ok");
    assert_eq!(response.protocol_version, PROTOCOL_VERSION);
}

#[test]
fn auth_headers_include_bearer_and_protocol_version() {
    let token_file = NamedTempFile::new().expect("token file");
    std::fs::write(token_file.path(), "test-token").expect("write token");
    let client = BelltowerClient::try_from("http://127.0.0.1:7400/")
        .expect("client")
        .with_auth_token_path(token_file.path());

    let headers = client.auth_headers().expect("auth headers");
    assert_eq!(
        headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer test-token")
    );
    assert_eq!(
        headers
            .get(PROTOCOL_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(PROTOCOL_VERSION)
    );
}

#[test]
fn explicit_remote_constructor_uses_static_token() {
    let client = BelltowerClient::new_with_auth_token(
        Url::parse("http://127.0.0.1:7400/").expect("url"),
        "remote-token",
    )
    .expect("client");

    let headers = client.auth_headers().expect("auth headers");
    assert_eq!(
        headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer remote-token")
    );
}

#[test]
fn auth_headers_reload_path_token_after_rotation() {
    let token_file = NamedTempFile::new().expect("token file");
    std::fs::write(token_file.path(), "first-token").expect("write token");
    let client = BelltowerClient::try_from("http://127.0.0.1:7400/")
        .expect("client")
        .with_auth_token_path(token_file.path());

    let first = client.auth_headers().expect("first auth headers");
    std::fs::write(token_file.path(), "second-token").expect("rewrite token");
    let second = client.auth_headers().expect("second auth headers");

    assert_eq!(
        first
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer first-token")
    );
    assert_eq!(
        second
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer second-token")
    );
}

#[test]
fn discovered_auth_token_paths_prefer_endpoint_record_before_legacy_path() {
    let base_url = Url::parse("http://127.0.0.1:7400/").expect("url");
    let paths = discovered_auth_token_paths(&base_url).expect("paths");
    let endpoint = server_auth_token_path_for_url(&base_url)
        .expect("endpoint path")
        .into_std_path_buf();

    assert_eq!(paths[0], endpoint);
    assert_eq!(paths[1], auth_token_path().into_std_path_buf());
}

#[test]
fn load_auth_token_from_paths_prefers_first_available_endpoint_token() {
    let endpoint_token = NamedTempFile::new().expect("endpoint token");
    let legacy_token = NamedTempFile::new().expect("legacy token");
    std::fs::write(endpoint_token.path(), "endpoint-token").expect("write endpoint token");
    std::fs::write(legacy_token.path(), "legacy-token").expect("write legacy token");

    let token = load_auth_token_from_paths(&[
        endpoint_token.path().to_path_buf(),
        legacy_token.path().to_path_buf(),
    ])
    .expect("load token");

    assert_eq!(token, "endpoint-token");
}

#[test]
fn load_auth_token_from_paths_skips_blank_endpoint_token_files() {
    let endpoint_token = NamedTempFile::new().expect("endpoint token");
    let legacy_token = NamedTempFile::new().expect("legacy token");
    std::fs::write(endpoint_token.path(), "   \n").expect("write blank endpoint token");
    std::fs::write(legacy_token.path(), "legacy-token").expect("write legacy token");

    let token = load_auth_token_from_paths(&[
        endpoint_token.path().to_path_buf(),
        legacy_token.path().to_path_buf(),
    ])
    .expect("load token");

    assert_eq!(token, "legacy-token");
}

#[test]
fn stream_headers_include_last_event_id() {
    let token_file = NamedTempFile::new().expect("token file");
    std::fs::write(token_file.path(), "test-token").expect("write token");
    let client = BelltowerClient::try_from("http://127.0.0.1:7400/")
        .expect("client")
        .with_auth_token_path(token_file.path());

    let headers = client.stream_headers(Some(42)).expect("stream headers");
    assert_eq!(
        headers
            .get("last-event-id")
            .and_then(|value| value.to_str().ok()),
        Some("42")
    );
    assert_eq!(
        headers
            .get(PROTOCOL_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(PROTOCOL_VERSION)
    );
}

#[test]
fn public_belltower_client_methods_have_consumer_mode_annotations() {
    const REMOTE: &str = "/// # Remote client-safe";
    const LAUNCHER: &str = "/// # Launcher / local TUI only";
    const INTERNAL: &str = "/// # Internal (do not use)";
    const EXPECTED_METHODS: &[&str] = &[
        "new",
        "new_discovering_auth",
        "new_with_auth_token",
        "new_with_auth_token_path",
        "with_auth_token_path",
        "with_auth_token",
        "health",
        "server_info",
        "create_session",
        "spawn_session",
        "list_sessions",
        "inspect_session",
        "update_session",
        "update_session_budget",
        "send_message",
        "create_branch",
        "compact_session",
        "activate_branch",
        "approve_tool",
        "answer_tool",
        "cancel_session",
        "steer_session",
        "record_operator_command",
        "run_shell_command",
        "session_events",
        "session_turns",
        "wait_for_session_settle",
        "session_execution",
        "session_queue",
        "clear_session_queue",
        "session_tool_call",
        "search_session",
        "session_lineage",
        "session_workflow",
        "inspect_branches",
        "session_tree",
        "stream_events",
        "session_messages",
        "branch_messages",
        "branch_operator_commands",
        "branch_messages_page",
        "branch_operator_commands_page",
        "branches",
        "raw_chunks",
        "turn_raw_chunks_page",
        "export_session",
        "push_otlp_export",
        "connections",
        "status_inspection",
        "mcp_servers",
        "mcp_inventory",
        "mcp_tools",
        "reload_mcp",
        "model_backends",
        "model_recommendations",
        "connection_models",
        "connection_model_inventory",
    ];

    let source = include_str!("lib.rs");
    let start = source
        .find("impl BelltowerClient {")
        .expect("BelltowerClient impl start");
    let end = source
        .find("impl TryFrom<&str> for BelltowerClient")
        .expect("BelltowerClient impl end");
    let block = &source[start..end];

    let mut annotated_methods = Vec::new();
    let mut last_mode: Option<&str> = None;

    for line in block.lines() {
        let trimmed = line.trim();
        match trimmed {
            REMOTE => last_mode = Some(REMOTE),
            LAUNCHER => last_mode = Some(LAUNCHER),
            INTERNAL => last_mode = Some(INTERNAL),
            _ => {
                let signature = trimmed
                    .strip_prefix("pub async fn ")
                    .or_else(|| trimmed.strip_prefix("pub fn "));
                if let Some(signature) = signature {
                    let method_name = signature.split('(').next().expect("method name").trim();
                    assert!(
                        last_mode.is_some(),
                        "public method `{method_name}` is missing a consumer-mode annotation"
                    );
                    annotated_methods.push(method_name.to_owned());
                    last_mode = None;
                }
            }
        }
    }

    assert_eq!(annotated_methods, EXPECTED_METHODS);
}

#[tokio::test]
async fn send_message_falls_back_to_runtime_api_errors_for_plain_text_bodies() {
    async fn fail() -> (StatusCode, &'static str) {
        (StatusCode::BAD_GATEWAY, "upstream exploded")
    }

    let app = Router::new().route("/sessions/{session_id}/message", post(fail));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let token_file = NamedTempFile::new().expect("token file");
    std::fs::write(token_file.path(), "test-token").expect("write token");
    let client = BelltowerClient::try_from(format!("http://{addr}/").as_str())
        .expect("client")
        .with_auth_token_path(token_file.path());

    let error = client
        .send_message(
            SessionId::new(),
            &SendMessageRequest {
                branch_id: BranchId::new(),
                message: Message::text(Role::User, "hello"),
            },
        )
        .await
        .expect_err("plain text api error");

    server.abort();

    match error {
        ClientError::Api {
            status,
            class,
            code,
            message,
            retryable,
        } => {
            assert_eq!(status, StatusCode::BAD_GATEWAY);
            assert_eq!(class, ErrorClass::Runtime);
            assert_eq!(code, None);
            assert_eq!(message, "upstream exploded");
            assert!(retryable);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn stream_events_fails_on_malformed_envelopes() {
    async fn stream() -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
        let events = vec![Event::default().data("{not-json}")];
        Sse::new(futures_util::stream::iter(
            events.into_iter().map(Ok::<_, Infallible>),
        ))
    }

    let app = Router::new().route("/sessions/{session_id}/events/stream", get(stream));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let token_file = NamedTempFile::new().expect("token file");
    std::fs::write(token_file.path(), "test-token").expect("write token");
    let client = BelltowerClient::try_from(format!("http://{addr}/").as_str())
        .expect("client")
        .with_auth_token_path(token_file.path());

    let mut stream = client
        .stream_events(SessionId::new(), None)
        .await
        .expect("event stream");
    let error = stream
        .next()
        .await
        .expect("stream frame")
        .expect_err("malformed frame should fail");

    server.abort();

    match error {
        ClientError::Core(bt_core::BelltowerError::Json(_)) => {}
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn control_surface_methods_cover_current_release_candidate_routes() {
    async fn create_session(
        Json(_request): Json<CreateSessionRequest>,
    ) -> Json<CreateSessionResponse> {
        Json(sample_create_session_response())
    }

    async fn update_session(
        Path(_session_id): Path<SessionId>,
        Json(_request): Json<UpdateSessionRequest>,
    ) -> Json<CreateSessionResponse> {
        Json(sample_create_session_response())
    }

    async fn workflow(Path(_session_id): Path<SessionId>) -> Json<SessionWorkflowResponse> {
        Json(sample_workflow_response())
    }

    async fn queue(Path(session_id): Path<SessionId>) -> Json<SessionQueueResponse> {
        Json(SessionQueueResponse {
            inspection: SessionQueueInspection {
                session_id,
                runtime_state: SessionRuntimeState::Working,
                cancel_requested: false,
                pending_steer_count: 0,
                queued_messages: vec![QueuedMessageInspection {
                    branch_id: BranchId::new(),
                    enqueued_at: time::OffsetDateTime::UNIX_EPOCH,
                    message: Message::text(Role::User, "queued follow-up"),
                    settings_revision_id: default_settings_revision_id(),
                }],
                pending_approvals: Vec::new(),
                pending_inputs: Vec::new(),
            },
        })
    }

    async fn clear_queue(Path(session_id): Path<SessionId>) -> Json<SessionQueueClearResponse> {
        Json(SessionQueueClearResponse {
            session_id,
            cleared_messages: vec![QueuedMessageInspection {
                branch_id: BranchId::new(),
                enqueued_at: time::OffsetDateTime::UNIX_EPOCH,
                message: Message::text(Role::User, "queued follow-up"),
                settings_revision_id: default_settings_revision_id(),
            }],
        })
    }

    async fn connections() -> Json<ConnectionsResponse> {
        Json(sample_connections_response())
    }

    async fn status() -> Json<StatusInspectionResponse> {
        Json(sample_status_response())
    }

    async fn model_backends() -> Json<ModelBackendsResponse> {
        Json(sample_model_backends_response())
    }

    async fn connection_models() -> Json<ConnectionModelsResponse> {
        Json(sample_connection_models_response())
    }

    async fn connection_model_inventory(
        Path(connection_id): Path<ConnectionId>,
    ) -> Json<ConnectionModelsResponse> {
        let mut response = sample_connection_models_response();
        response
            .connections
            .retain(|connection| connection.connection_id == connection_id);
        Json(response)
    }

    async fn mcp_servers() -> Json<McpServersResponse> {
        Json(sample_mcp_servers_response())
    }

    async fn mcp_inventory() -> Json<McpInventoryResponse> {
        Json(sample_mcp_inventory_response())
    }

    async fn mcp_tools() -> Json<McpToolsResponse> {
        Json(sample_mcp_tools_response())
    }

    async fn model_recommendations() -> Json<ModelRecommendationsResponse> {
        Json(sample_model_recommendations_response())
    }

    async fn server_info() -> Json<ServerInfoResponse> {
        Json(sample_server_info_response())
    }

    let app = Router::new()
        .route("/sessions", post(create_session).get(connections))
        .route("/sessions/{session_id}", post(update_session))
        .route("/sessions/{session_id}/queue", get(queue))
        .route("/sessions/{session_id}/queue/clear", post(clear_queue))
        .route("/sessions/{session_id}/workflow", get(workflow))
        .route("/connections", get(connections))
        .route("/server/info", get(server_info))
        .route("/status/inspect", get(status))
        .route("/models/backends", get(model_backends))
        .route("/models/connections", get(connection_models))
        .route(
            "/models/connections/{connection_id}",
            get(connection_model_inventory),
        )
        .route("/models/recommendations", get(model_recommendations))
        .route("/mcp", get(mcp_inventory))
        .route("/mcp/servers", get(mcp_servers))
        .route("/mcp/tools", get(mcp_tools));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let token_file = NamedTempFile::new().expect("token file");
    std::fs::write(token_file.path(), "test-token").expect("write token");
    let client = BelltowerClient::try_from(format!("http://{addr}/").as_str())
        .expect("client")
        .with_auth_token_path(token_file.path());

    let created = client
        .create_session(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("openai"),
            model_id: Some("gpt-5.1".to_owned()),
            tool_mode: Some(SessionToolMode::Extended),
            display_name: Some("fixture".to_owned()),
            objective: Some("exercise client routes".to_owned()),
            budget: None,
        })
        .await
        .expect("create session");
    let _updated = client
        .update_session(
            created.session.session_id,
            &UpdateSessionRequest {
                branch_id: created.branch.branch_id,
                connection_id: Some(ConnectionId::new("local")),
                model_id: Some("qwen2.5-coder:7b".to_owned()),
                tool_mode: Some(SessionToolMode::Standard),
                reset_model_to_default: false,
            },
        )
        .await
        .expect("update session");
    let _workflow = client
        .session_workflow(created.session.session_id)
        .await
        .expect("workflow");
    let _queue = client
        .session_queue(created.session.session_id)
        .await
        .expect("queue");
    let _cleared_queue = client
        .clear_session_queue(created.session.session_id)
        .await
        .expect("clear queue");
    let _connections = client.connections().await.expect("connections");
    let _server_info = client.server_info().await.expect("server info");
    let _status = client.status_inspection().await.expect("status");
    let _backends = client.model_backends().await.expect("model backends");
    let _connection_models = client.connection_models().await.expect("connection models");
    let _connection_model_inventory = client
        .connection_model_inventory(&ConnectionId::new("local"))
        .await
        .expect("connection model inventory");
    let _recommendations = client
        .model_recommendations()
        .await
        .expect("model recommendations");
    let _mcp_inventory = client.mcp_inventory().await.expect("mcp inventory");
    let _mcp_servers = client.mcp_servers().await.expect("mcp servers");
    let _mcp_tools = client.mcp_tools().await.expect("mcp tools");

    server.abort();
}

fn sample_create_session_response() -> CreateSessionResponse {
    let session_id = SessionId::new();
    CreateSessionResponse {
        session: SessionRecord {
            session_id,
            project_root: "/tmp/project".into(),
            connection_id: ConnectionId::new("openai"),
            model_id: Some("gpt-5.1".to_owned()),
            tool_mode: SessionToolMode::Extended,
            settings_revision_id: default_settings_revision_id(),
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
            status: SessionStatus::Active,
            display_name: Some("fixture".to_owned()),
            objective: Some("exercise client routes".to_owned()),
            parent_session_id: None,
            parent_branch_id: None,
            parent_turn_id: None,
        },
        branch: BranchRecord {
            branch_id: BranchId::new(),
            session_id,
            parent_branch_id: None,
            parent_event_id: None,
            head_event_id: None,
            summary: None,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            is_default: true,
        },
    }
}

fn sample_workflow_response() -> SessionWorkflowResponse {
    let created = sample_create_session_response();
    SessionWorkflowResponse {
        inspection: SessionWorkflowInspection {
            focus_session_id: created.session.session_id,
            root_session_id: created.session.session_id,
            node_count: 1,
            runtime_counts: WorkflowRuntimeCounts {
                idle: 1,
                working: 0,
                waiting_on_input: 0,
                waiting_on_approval: 0,
                cancel_requested: 0,
            },
            status_counts: WorkflowStatusCounts {
                active: 1,
                completed: 0,
                failed: 0,
                abandoned: 0,
            },
            nodes: vec![WorkflowSessionNode {
                session: created.session,
                depth: 0,
                is_focus: true,
                runtime_state: SessionRuntimeState::Idle,
                active_branch_id: Some(created.branch.branch_id),
                turn_count: 0,
                message_count: 0,
                tool_call_count: 0,
                pending_approval_count: 0,
                child_session_count: 0,
                last_seq_id: None,
            }],
        },
    }
}

fn sample_connections_response() -> ConnectionsResponse {
    ConnectionsResponse {
        connections: vec![ConnectionDescriptor {
            id: ConnectionId::new("openai"),
            provider: "openai".to_owned(),
            base_url: Url::parse("https://api.openai.com/v1/").expect("url"),
            default_model: "gpt-5.1".to_owned(),
            auth_methods: Vec::new(),
            auth_sources: vec!["auth_store".to_owned()],
            model_fallbacks: vec!["gpt-5-mini".to_owned()],
            discoverable_model_selectors: Vec::new(),
        }],
    }
}

fn sample_status_response() -> StatusInspectionResponse {
    StatusInspectionResponse {
        inspection: StatusInspection {
            auth_storage: "auth store (/tmp/auth.toml)".to_owned(),
            default_connection: ConnectionId::new("openai"),
            connections: vec![ConnectionReadinessInspection {
                connection_id: ConnectionId::new("openai"),
                provider: "openai".to_owned(),
                default_model: "gpt-5.1".to_owned(),
                auth_methods: Vec::new(),
                auth_state: ConnectionAuthState::Configured,
                auth_kind: Some(CredentialKind::ApiKey),
                auth_source: Some("auth_store".to_owned()),
                auth_tried_sources: Vec::new(),
                support_state: ConnectionSupportState::RuntimeSupported,
                readiness_state: ConnectionReadinessState::Ready,
                probe_ready: true,
                probe_status: "ok".to_owned(),
            }],
        },
    }
}

fn sample_server_info_response() -> ServerInfoResponse {
    ServerInfoResponse {
        server_version: "0.1.0".to_owned(),
        protocol_version: PROTOCOL_VERSION.to_owned(),
        supported_protocol_versions: vec![PROTOCOL_VERSION.to_owned()],
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
    }
}

fn sample_model_backends_response() -> ModelBackendsResponse {
    ModelBackendsResponse {
        backends: vec![ModelBackendDescriptor {
            kind: ModelBackendKind::Ollama,
            label: "Ollama".to_owned(),
            base_url: Url::parse("http://127.0.0.1:11434/").expect("url"),
            status: ModelBackendStatus::Ready,
            available_models: vec!["qwen2.5-coder:7b".to_owned()],
        }],
    }
}

fn sample_connection_models_response() -> ConnectionModelsResponse {
    ConnectionModelsResponse {
        connections: vec![ConnectionModelInventory {
            connection_id: ConnectionId::new("openai"),
            provider: "openai".to_owned(),
            default_model: "gpt-5.1".to_owned(),
            auth_methods: Vec::new(),
            auth_state: ConnectionAuthState::Configured,
            auth_source: Some("auth_store".to_owned()),
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
            ],
        }],
    }
}

fn sample_mcp_servers_response() -> McpServersResponse {
    McpServersResponse {
        servers: vec![McpServerDescriptor {
            name: "docs".to_owned(),
            transport: McpTransportKind::StreamableHttp,
            enabled: true,
            status: McpServerStatus::Ready,
        }],
    }
}

fn sample_mcp_inventory_response() -> McpInventoryResponse {
    McpInventoryResponse {
        servers: sample_mcp_servers_response().servers,
        tools: sample_mcp_tools_response().tools,
    }
}

fn sample_mcp_tools_response() -> McpToolsResponse {
    McpToolsResponse {
        tools: vec![McpToolDescriptor {
            server_name: "docs".to_owned(),
            tool_name: "search_docs".to_owned(),
            qualified_name: "docs.search_docs".to_owned(),
            description: "Search docs".to_owned(),
            parameters_schema: json!({
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": {"type": "string"}
                }
            }),
        }],
    }
}

fn sample_model_recommendations_response() -> ModelRecommendationsResponse {
    ModelRecommendationsResponse {
        report: ModelRecommendationsReport {
            hardware: bt_core::HardwareProfile {
                total_memory_gb: Some(64),
            },
            recommendations: vec![ModelRecommendation {
                label: "Laptop".to_owned(),
                max_memory_gb: 64,
                models: vec!["qwen2.5-coder:7b".to_owned()],
                fits_hardware: true,
            }],
        },
    }
}
