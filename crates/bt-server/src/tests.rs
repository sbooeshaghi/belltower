//! Tests for bt-server HTTP contracts, SSE replay, exports, prompts, and helper behavior.

use super::{
    AppState, build_app, build_session_system_prompt_with_provenance, build_tool_registry,
    queue_message_input, raw_sse_payload, status_for_error, write_auth_token,
};
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post};
use axum::{Json, Router, extract::State};
use base64::Engine;
use bt_client::{BelltowerClient, ClientError};
use bt_core::{
    ApprovalDecision, ApprovalDecisionSource, ApprovalScope, BelltowerConfig, BelltowerError,
    BudgetConfig, CompletionDelta, CompletionSummary, ConnectionDescriptor, ConnectionId,
    ContextCompactionPhase, ContextCompactionStatus, ContextCompactionTrigger, CostBreakdown,
    ErrorClass, EventEnvelope, EventPayload, FinishReason, McpServerConfig, McpServerStatus,
    McpTransportConfig, Message, MessagePart, ModelBackendStatus, Role, SessionId,
    SessionRuntimeState, SessionToolMode, TokenUsage, ToolCall, ToolCallId, ToolOperationInitiator,
    ToolResultEnvelope, TurnId, TurnStartSource,
};
use bt_protocol::{
    ActivateBranchRequest, AnswerToolRequest, ApproveToolRequest, BranchMessagesPageResponse,
    BranchOperatorCommandsPageResponse, BranchOperatorCommandsResponse, BranchesResponse,
    CancelSessionRequest, CompactSessionRequest, CompactSessionResponse, ConnectionModelsResponse,
    CreateBranchRequest, CreateBranchResponse, CreateSessionRequest, CreateSessionResponse,
    ErrorEnvelope, ExportFormat, McpInventoryResponse, McpServersResponse, McpToolsResponse,
    ModelBackendsResponse, ModelRecommendationsResponse, PROTOCOL_HEADER, PROTOCOL_VERSION,
    PushOtlpExportRequest, PushOtlpExportResponse, RawSseEnvelope, RecordOperatorCommandRequest,
    RunShellCommandRequest, SendMessageOutcome, SendMessageRequest, SendMessageResponse,
    ServerInfoResponse, SessionBranchInspectionResponse, SessionEventsResponse,
    SessionExecutionResponse, SessionExportResponse, SessionInspectionResponse,
    SessionMessagesResponse, SessionQueueClearResponse, SessionQueueResponse,
    SessionRawChunksResponse, SessionToolCallResponse, SessionTreeResponse, SessionTurnsResponse,
    SessionWorkflowResponse, SpawnSessionRequest, SpawnSessionResponse, SteerSessionRequest,
    TurnRawChunksPageResponse, UpdateSessionBudgetRequest, UpdateSessionRequest,
};
use bt_providers::SseParser;
use bt_runtime::BelltowerRuntime;
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTracePartialSuccess, ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use prost::Message as _;
use reqwest::{Client, Method, Response, StatusCode};
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

struct TestServer {
    base_url: String,
    token: String,
    runtime: Arc<BelltowerRuntime>,
    _temp_dir: Option<TempDir>,
    handle: JoinHandle<()>,
}

impl TestServer {
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn request(&self, client: &Client, method: Method, path: &str) -> reqwest::RequestBuilder {
        client
            .request(method, self.url(path))
            .bearer_auth(&self.token)
            .header(PROTOCOL_HEADER, PROTOCOL_VERSION)
    }
}

fn api_client(server: &TestServer) -> BelltowerClient {
    BelltowerClient::new_with_auth_token(
        server.base_url.parse().expect("server base url"),
        server.token.clone(),
    )
    .expect("api client")
}

/// Message dispatch is asynchronous: send/approve/answer return 202 once the
/// turn is durably admitted, before the turn runs. Tests call this after any
/// dispatching POST to wait until the runtime settles (no running turn, no
/// queued messages) before observing messages/events/turns/exports.
async fn settle(api: &BelltowerClient, session_id: SessionId) -> SessionRuntimeState {
    api.wait_for_session_settle(session_id, std::time::Duration::from_secs(15))
        .await
        .expect("session settles")
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

struct MockProvider {
    base_url: String,
    handle: JoinHandle<()>,
    requests: Option<Arc<Mutex<Vec<Value>>>>,
}

impl Drop for MockProvider {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

struct ControlledStreamProvider {
    base_url: String,
    request_seen: tokio::sync::watch::Receiver<bool>,
    release_stream: tokio::sync::watch::Sender<bool>,
    handle: JoinHandle<()>,
}

impl Drop for ControlledStreamProvider {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

#[derive(Clone)]
struct ControlledStreamProviderState {
    request_seen: tokio::sync::watch::Sender<bool>,
    release_stream: tokio::sync::watch::Sender<bool>,
}

#[derive(Clone)]
struct CapturedOtlpRequest {
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

#[derive(Clone)]
struct MockOtlpCollectorResponse {
    status: StatusCode,
    content_type: Option<String>,
    body: Vec<u8>,
}

impl MockOtlpCollectorResponse {
    fn empty_ok() -> Self {
        Self {
            status: StatusCode::OK,
            content_type: None,
            body: Vec::new(),
        }
    }

    fn protobuf_ok(body: Vec<u8>) -> Self {
        Self {
            status: StatusCode::OK,
            content_type: Some("application/x-protobuf".to_owned()),
            body,
        }
    }

    fn json_ok(body: Vec<u8>) -> Self {
        Self {
            status: StatusCode::OK,
            content_type: Some("application/json".to_owned()),
            body,
        }
    }
}

#[test]
fn write_auth_token_creates_parent_directories() {
    let temp_dir = TempDir::new().expect("temp dir");
    let token_path = temp_dir
        .path()
        .join("server-auth/http_127_0_0_1_7400.token");

    write_auth_token(&token_path, "test-token").expect("write auth token");

    assert_eq!(
        std::fs::read_to_string(&token_path).expect("read token"),
        "test-token"
    );
}

struct MockOtlpCollector {
    base_url: String,
    captured: Arc<Mutex<Option<CapturedOtlpRequest>>>,
    handle: JoinHandle<()>,
}

impl Drop for MockOtlpCollector {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

struct SseReader {
    response: Response,
    parser: SseParser,
    buffered: VecDeque<bt_providers::SseEvent>,
}

impl SseReader {
    fn new(response: Response) -> Self {
        Self {
            response,
            parser: SseParser::new(),
            buffered: VecDeque::new(),
        }
    }

    async fn next(&mut self) -> RawSseEnvelope {
        loop {
            if let Some(event) = self.buffered.pop_front() {
                return serde_json::from_str(&event.data).expect("valid raw sse envelope");
            }

            let chunk = self
                .response
                .chunk()
                .await
                .expect("stream chunk")
                .expect("stream should stay open");
            self.buffered.extend(self.parser.push(chunk.as_ref()));
        }
    }
}

async fn assert_error_response(
    response: Response,
    status: StatusCode,
    class: ErrorClass,
) -> ErrorEnvelope {
    assert_eq!(response.status(), status);
    assert_eq!(
        response
            .headers()
            .get(PROTOCOL_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(PROTOCOL_VERSION)
    );
    let envelope: ErrorEnvelope = response.json().await.expect("error envelope");
    assert_eq!(envelope.class, class);
    assert!(!envelope.code.is_empty());
    envelope
}

fn assert_client_api_error(error: ClientError, status: StatusCode, class: ErrorClass) {
    match error {
        ClientError::Api {
            status: actual_status,
            class: actual_class,
            code,
            message,
            ..
        } => {
            assert_eq!(actual_status, status);
            assert_eq!(actual_class, class);
            assert!(code.as_deref().is_some_and(|value| !value.is_empty()));
            assert!(!message.is_empty());
        }
        other => panic!("unexpected client error: {other:?}"),
    }
}

#[tokio::test]
async fn health_is_public_and_protected_routes_require_auth() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();

    let health = client
        .get(server.url("health"))
        .send()
        .await
        .expect("health request");
    assert_eq!(health.status(), StatusCode::OK);

    let protected = client
        .get(server.url("sessions"))
        .send()
        .await
        .expect("protected request");
    assert_eq!(protected.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        protected
            .headers()
            .get(PROTOCOL_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(PROTOCOL_VERSION)
    );
    let envelope: ErrorEnvelope = protected.json().await.expect("auth envelope");
    assert_eq!(envelope.class, ErrorClass::Auth);
    assert_eq!(envelope.code, "unauthorized");

    let metadata = client
        .get(server.url("server/info"))
        .send()
        .await
        .expect("server info request");
    assert_eq!(metadata.status(), StatusCode::UNAUTHORIZED);

    let bad_token = client
        .get(server.url("sessions"))
        .bearer_auth("wrong-token")
        .send()
        .await
        .expect("bad token request");
    assert_eq!(bad_token.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        bad_token
            .headers()
            .get(PROTOCOL_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(PROTOCOL_VERSION)
    );

    let with_auth = client
        .get(server.url("sessions"))
        .bearer_auth(&server.token)
        .send()
        .await
        .expect("authorized request");
    assert_eq!(with_auth.status(), StatusCode::OK);
}

#[tokio::test]
async fn server_info_reports_versions_and_capabilities() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();

    let info: ServerInfoResponse = server
        .request(&client, Method::GET, "server/info")
        .send()
        .await
        .expect("server info request")
        .json()
        .await
        .expect("server info body");
    assert_eq!(info.server_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(info.protocol_version, PROTOCOL_VERSION);
    assert_eq!(
        info.supported_protocol_versions,
        vec![PROTOCOL_VERSION.to_owned()]
    );
    assert!(info.capabilities.approvals);
    assert!(info.capabilities.pending_input);
    assert!(info.capabilities.session_queue);
    assert!(info.capabilities.workflow_inspection);
    assert!(info.capabilities.lineage_inspection);
    assert!(info.capabilities.raw_chunk_paging);
    assert!(info.capabilities.exports);
    assert!(info.capabilities.mcp_inventory);
    assert!(info.capabilities.mcp_reload);
    assert!(info.capabilities.spawn_session);
}

#[test]
fn api_error_status_tracks_error_class() {
    assert_eq!(
        status_for_error(&BelltowerError::Config("bad config".to_owned())),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status_for_error(&BelltowerError::Auth("bad auth".to_owned())),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status_for_error(&BelltowerError::Provider("provider down".to_owned())),
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(
        status_for_error(&BelltowerError::Protocol("bad request".to_owned())),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status_for_error(&BelltowerError::Tool("bad tool args".to_owned())),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status_for_error(&BelltowerError::Storage("disk full".to_owned())),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        status_for_error(&BelltowerError::NotFound("missing".to_owned())),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn protocol_negotiation_accepts_current_or_missing_version_and_rejects_unknown() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();

    let missing_version = client
        .get(server.url("sessions"))
        .bearer_auth(&server.token)
        .send()
        .await
        .expect("missing protocol header request");
    assert_eq!(missing_version.status(), StatusCode::OK);

    let current_version = client
        .get(server.url("sessions"))
        .bearer_auth(&server.token)
        .header(PROTOCOL_HEADER, PROTOCOL_VERSION)
        .send()
        .await
        .expect("current protocol header request");
    assert_eq!(current_version.status(), StatusCode::OK);

    let unsupported = client
        .get(server.url("sessions"))
        .bearer_auth(&server.token)
        .header(PROTOCOL_HEADER, "belltower.v999")
        .send()
        .await
        .expect("unsupported protocol request");
    assert_eq!(unsupported.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        unsupported
            .headers()
            .get(PROTOCOL_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(PROTOCOL_VERSION)
    );
    let envelope: ErrorEnvelope = unsupported.json().await.expect("protocol envelope");
    assert_eq!(envelope.class, ErrorClass::Protocol);
    assert_eq!(envelope.code, "unsupported_protocol_version");
}

#[tokio::test]
async fn api_errors_include_protocol_header_and_typed_envelope() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();

    let response = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}", SessionId::new()),
        )
        .send()
        .await
        .expect("api error response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response
            .headers()
            .get(PROTOCOL_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(PROTOCOL_VERSION)
    );
    let envelope: ErrorEnvelope = response.json().await.expect("api error envelope");
    assert_eq!(envelope.class, ErrorClass::NotFound);
    assert_eq!(envelope.code, "not_found");
}

// 5.4 scenario -> ErrorClass mapping:
// bad auth -> Auth; missing session -> NotFound; malformed request -> Protocol;
// protocol-version mismatch -> Protocol; storage-unavailable simulation -> Storage.
// Storage failures during a dispatched turn are observed durably (session.error
// event + failed turn) rather than as an HTTP status on the send POST.
#[tokio::test]
async fn error_envelope_matrix_pins_client_visible_error_classes() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();
    let api = api_client(&server);

    let bad_auth_client =
        BelltowerClient::new_with_auth_token(server.base_url.parse().expect("url"), "bad-token")
            .expect("bad auth client");
    assert_client_api_error(
        bad_auth_client
            .list_sessions()
            .await
            .expect_err("bad auth should fail"),
        StatusCode::UNAUTHORIZED,
        ErrorClass::Auth,
    );

    assert_client_api_error(
        api.inspect_session(SessionId::new())
            .await
            .expect_err("missing session should fail"),
        StatusCode::NOT_FOUND,
        ErrorClass::NotFound,
    );

    let malformed_request = client
        .post(server.url("sessions"))
        .bearer_auth(&server.token)
        .header(PROTOCOL_HEADER, PROTOCOL_VERSION)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body("{")
        .send()
        .await
        .expect("malformed request");
    assert_error_response(
        malformed_request,
        StatusCode::BAD_REQUEST,
        ErrorClass::Protocol,
    )
    .await;

    let protocol_mismatch = client
        .get(server.url("sessions"))
        .bearer_auth(&server.token)
        .header(PROTOCOL_HEADER, "belltower.v999")
        .send()
        .await
        .expect("protocol mismatch request");
    assert_error_response(
        protocol_mismatch,
        StatusCode::BAD_REQUEST,
        ErrorClass::Protocol,
    )
    .await;

    let mock_provider = spawn_controlled_stream_provider().await;
    let storage_server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let storage_api = api_client(&storage_server);
    let created = storage_api
        .create_session(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("error-matrix-storage".to_owned()),
            objective: None,
            budget: None,
        })
        .await
        .expect("create storage session");

    let sent = storage_api
        .send_message(
            created.session.session_id,
            &SendMessageRequest {
                branch_id: created.branch.branch_id,
                message: Message::text(Role::User, "hello"),
            },
        )
        .await
        .expect("send message");
    assert_eq!(sent.outcome, SendMessageOutcome::Dispatched);

    let mut request_seen = mock_provider.request_seen.clone();
    while !*request_seen.borrow_and_update() {
        request_seen
            .changed()
            .await
            .expect("provider request starts");
    }

    storage_server
        .runtime
        .inject_next_store_append_error_for_test("matrix storage failure");
    mock_provider
        .release_stream
        .send(true)
        .expect("release provider stream");

    settle(&storage_api, created.session.session_id).await;
    let turns = storage_api
        .session_turns(created.session.session_id)
        .await
        .expect("session turns");
    assert_eq!(
        turns.turns.last().and_then(|turn| turn.status.as_deref()),
        Some("failed")
    );
    let events = storage_api
        .session_events(created.session.session_id, None)
        .await
        .expect("session events");
    assert!(events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::SessionError { class, code, .. }
            if *class == ErrorClass::Storage && code == "storage_error"
    )));
}

#[tokio::test]
async fn missing_session_read_routes_return_not_found() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();
    let missing_session_id = SessionId::new();

    let paths = vec![
        format!("sessions/{missing_session_id}"),
        format!("sessions/{missing_session_id}/events"),
        format!("sessions/{missing_session_id}/turns"),
        format!("sessions/{missing_session_id}/messages"),
        format!("sessions/{missing_session_id}/raw_chunks"),
        format!("sessions/{missing_session_id}/export/legacy-bundle"),
    ];

    for path in paths {
        let response = server
            .request(&client, Method::GET, &path)
            .send()
            .await
            .unwrap_or_else(|error| panic!("request {path} failed: {error}"));
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        let envelope: ErrorEnvelope = response.json().await.expect("error envelope");
        assert_eq!(envelope.class, ErrorClass::NotFound, "{path}");
        assert_eq!(envelope.code, "not_found", "{path}");
    }
}

#[tokio::test]
async fn approve_missing_tool_call_returns_not_found() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();
    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("openai"),
            model_id: None,
            tool_mode: None,
            display_name: Some("approve-missing".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let response = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/approve", created.session.session_id),
        )
        .json(&ApproveToolRequest {
            call_id: ToolCallId::new("missing-call"),
            tool_name: "shell".to_owned(),
            scope: bt_core::ApprovalScope::Once,
            decision: bt_core::ApprovalDecision::Approved {
                decided_at: time::OffsetDateTime::UNIX_EPOCH,
                decided_by: "tester".to_owned(),
                scope: bt_core::ApprovalScope::Once,
                source: bt_core::ApprovalDecisionSource::Human,
            },
        })
        .send()
        .await
        .expect("approve request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let envelope: ErrorEnvelope = response.json().await.expect("approve error");
    assert_eq!(envelope.class, ErrorClass::NotFound);
    assert_eq!(envelope.code, "not_found");
}

#[tokio::test]
async fn approve_with_wrong_tool_name_returns_not_found_and_keeps_pending_approval() {
    let mock_provider = spawn_mock_approval_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("approve-wrong-tool".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let send = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "run a command"),
        })
        .send()
        .await
        .expect("send message");
    assert_eq!(send.status(), StatusCode::ACCEPTED);
    assert_eq!(
        settle(&api_client(&server), created.session.session_id).await,
        SessionRuntimeState::WaitingOnApproval
    );

    let response = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/approve", created.session.session_id),
        )
        .json(&ApproveToolRequest {
            call_id: ToolCallId::new("call-approval"),
            tool_name: "read".to_owned(),
            scope: bt_core::ApprovalScope::Once,
            decision: bt_core::ApprovalDecision::Approved {
                decided_at: time::OffsetDateTime::UNIX_EPOCH,
                decided_by: "tester".to_owned(),
                scope: bt_core::ApprovalScope::Once,
                source: bt_core::ApprovalDecisionSource::Human,
            },
        })
        .send()
        .await
        .expect("approve request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let queue: SessionQueueResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/queue", created.session.session_id),
        )
        .send()
        .await
        .expect("queue request")
        .json()
        .await
        .expect("queue body");
    assert_eq!(queue.inspection.pending_approvals.len(), 1);
    assert_eq!(queue.inspection.pending_approvals[0].tool_name, "shell");
}

#[tokio::test]
async fn route_inventory_sanity_covers_current_release_candidate_surface() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();
    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("openai"),
            model_id: None,
            tool_mode: None,
            display_name: Some("route-sanity".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let session_id = created.session.session_id;
    let branch_id = created.branch.branch_id;

    let checks = vec![
        (Method::GET, "sessions".to_owned()),
        (Method::GET, "server/info".to_owned()),
        (Method::GET, format!("sessions/{session_id}")),
        (Method::POST, format!("sessions/{session_id}/budget")),
        (Method::GET, format!("sessions/{session_id}/events")),
        (Method::GET, format!("sessions/{session_id}/turns")),
        (Method::GET, format!("sessions/{session_id}/execution")),
        (Method::GET, format!("sessions/{session_id}/queue")),
        (Method::POST, format!("sessions/{session_id}/queue/clear")),
        (Method::GET, format!("sessions/{session_id}/lineage")),
        (Method::GET, format!("sessions/{session_id}/workflow")),
        (Method::GET, format!("sessions/{session_id}/tree")),
        (Method::GET, format!("sessions/{session_id}/messages")),
        (
            Method::GET,
            format!("sessions/{session_id}/branches/{branch_id}/messages"),
        ),
        (
            Method::GET,
            format!("sessions/{session_id}/branches/{branch_id}/commands"),
        ),
        (Method::GET, "connections".to_owned()),
        (Method::GET, "status/inspect".to_owned()),
        (Method::GET, "mcp".to_owned()),
        (Method::GET, "mcp/servers".to_owned()),
        (Method::GET, "mcp/tools".to_owned()),
        (Method::POST, "mcp/reload".to_owned()),
        (Method::GET, "models/backends".to_owned()),
        (Method::GET, "models/connections".to_owned()),
        (Method::GET, "models/recommendations".to_owned()),
    ];

    for (method, path) in checks {
        let response = server
            .request(&client, method.clone(), &path)
            .send()
            .await
            .unwrap_or_else(|error| panic!("request {method} {path} failed: {error}"));
        assert_ne!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
        assert_ne!(
            response.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} {path}"
        );
    }
}

#[test]
fn openapi_document_is_generated_from_server_routes_and_protocol_schemas() {
    let document = super::openapi::openapi_document();
    assert_eq!(document["openapi"], "3.1.0");

    let paths = document["paths"].as_object().expect("paths object");
    let route_pairs: BTreeSet<_> = super::openapi::route_specs()
        .iter()
        .map(|route| {
            (
                route.path.to_owned(),
                route.method.as_str().to_owned(),
                route.request.map(str::to_owned),
                route.response.map(str::to_owned),
            )
        })
        .collect();
    let document_pairs: BTreeSet<_> = paths
        .iter()
        .flat_map(|(path, methods)| {
            methods
                .as_object()
                .expect("path item object")
                .keys()
                .map(|method| {
                    let operation = &methods[method];
                    let request = operation
                        .get("requestBody")
                        .and_then(|request| {
                            request.pointer("/content/application~1json/schema/$ref")
                        })
                        .and_then(Value::as_str)
                        .map(schema_name_from_ref);
                    let response = operation
                        .get("responses")
                        .and_then(|responses| {
                            responses
                                .get("200")
                                .or_else(|| responses.get("202"))
                                .and_then(|response| {
                                    response
                                        .pointer("/content/application~1json/schema/$ref")
                                        .or_else(|| {
                                            response
                                                .pointer("/content/text~1event-stream/schema/$ref")
                                        })
                                })
                        })
                        .and_then(Value::as_str)
                        .map(schema_name_from_ref);
                    (path.clone(), method.clone(), request, response)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(document_pairs, route_pairs);

    let schemas = document
        .pointer("/components/schemas")
        .and_then(Value::as_object)
        .expect("schema components");
    let mut refs = Vec::new();
    collect_schema_refs(&document, &mut refs);
    for reference in refs {
        let Some(name) = reference.strip_prefix("#/components/schemas/") else {
            panic!("unexpected external schema ref: {reference}");
        };
        assert!(
            schemas.contains_key(name),
            "missing OpenAPI schema component for {name}"
        );
    }

    assert!(
        document
            .pointer("/paths/~1sessions~1{session_id}~1message/post/responses/202/content/application~1json/schema/$ref")
            .is_some()
    );
    assert!(
        document
            .pointer("/paths/~1sessions~1{session_id}~1events~1stream/get/responses/200/content/text~1event-stream/schema/$ref")
            .is_some()
    );
}

fn schema_name_from_ref(reference: &str) -> String {
    reference
        .strip_prefix("#/components/schemas/")
        .unwrap_or(reference)
        .to_owned()
}

fn collect_schema_refs(value: &Value, refs: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(reference) = map.get("$ref").and_then(Value::as_str) {
                refs.push(reference.to_owned());
            }
            for child in map.values() {
                collect_schema_refs(child, refs);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_schema_refs(child, refs);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

#[tokio::test]
async fn mcp_routes_report_configured_servers() {
    let mut config = BelltowerConfig::from_embedded().expect("config");
    config.mcp.servers = vec![McpServerConfig {
        name: "fixture".to_owned(),
        enabled: false,
        transport: McpTransportConfig::Stdio {
            command: "/bin/false".to_owned(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
        },
    }];
    let server = spawn_server(config).await;
    let client = Client::new();

    let listed: McpServersResponse = server
        .request(&client, Method::GET, "mcp/servers")
        .send()
        .await
        .expect("mcp servers")
        .json()
        .await
        .expect("mcp servers body");
    assert_eq!(listed.servers.len(), 1);
    assert_eq!(listed.servers[0].name, "fixture");
    assert!(matches!(
        listed.servers[0].status,
        McpServerStatus::Configured
    ));

    let inventory: McpInventoryResponse = server
        .request(&client, Method::GET, "mcp")
        .send()
        .await
        .expect("mcp inventory")
        .json()
        .await
        .expect("mcp inventory body");
    assert_eq!(inventory.servers.len(), 1);
    assert!(matches!(
        inventory.servers[0].status,
        McpServerStatus::Configured
    ));
    assert!(inventory.tools.is_empty());

    let tools: McpToolsResponse = server
        .request(&client, Method::GET, "mcp/tools")
        .send()
        .await
        .expect("mcp tools")
        .json()
        .await
        .expect("mcp tools body");
    assert!(tools.tools.is_empty());

    let reloaded = server
        .request(&client, Method::POST, "mcp/reload")
        .send()
        .await
        .expect("mcp reload");
    assert_eq!(reloaded.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn model_routes_report_backends_and_recommendations() {
    async fn ollama_models() -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "models": [
                { "name": "qwen2.5-coder:7b" },
                { "name": "phi4-mini" }
            ]
        }))
    }

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind model backend");
    let addr = listener.local_addr().expect("model backend addr");
    let handle = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/api/tags", get(ollama_models)),
        )
        .await
        .expect("model backend should run");
    });

    let mut config = BelltowerConfig::from_embedded().expect("config");
    config.models.backends[0].base_url = format!("http://{addr}").parse().expect("backend url");
    config.models.backends[1].enabled = false;
    config.models.backends[2].enabled = false;
    let server = spawn_server(config).await;
    let client = Client::new();

    let backends: ModelBackendsResponse = server
        .request(&client, Method::GET, "models/backends")
        .send()
        .await
        .expect("model backends")
        .json()
        .await
        .expect("model backends body");
    assert_eq!(backends.backends.len(), 3);
    assert!(matches!(
        backends.backends[0].status,
        ModelBackendStatus::Ready
    ));
    assert_eq!(
        backends.backends[0].available_models,
        vec!["qwen2.5-coder:7b", "phi4-mini"]
    );
    assert!(matches!(
        backends.backends[1].status,
        ModelBackendStatus::Disabled
    ));

    let recommendations: ModelRecommendationsResponse = server
        .request(&client, Method::GET, "models/recommendations")
        .send()
        .await
        .expect("model recommendations")
        .json()
        .await
        .expect("model recommendations body");
    assert!(!recommendations.report.recommendations.is_empty());

    let connection_models: ConnectionModelsResponse = server
        .request(&client, Method::GET, "models/connections")
        .send()
        .await
        .expect("connection models")
        .json()
        .await
        .expect("connection models body");
    let local = connection_models
        .connections
        .iter()
        .find(|connection| connection.connection_id == ConnectionId::new("local"))
        .expect("local connection models");
    assert!(local.probe_ready);
    assert!(
        local
            .models
            .iter()
            .any(|model| model.model_id == "qwen2.5-coder:7b")
    );
    assert!(
        local
            .models
            .iter()
            .any(|model| model.model_id == "gpt-oss:20b")
    );

    let targeted: ConnectionModelsResponse = server
        .request(&client, Method::GET, "models/connections/local")
        .send()
        .await
        .expect("targeted connection models")
        .json()
        .await
        .expect("targeted connection models body");
    assert_eq!(targeted.connections.len(), 1);
    assert_eq!(
        targeted.connections[0].connection_id,
        ConnectionId::new("local")
    );
    assert!(targeted.connections[0].probe_ready);

    handle.abort();
}

#[tokio::test]
async fn session_round_trip_persists_messages_events_and_raw_chunks() {
    let mock_provider = spawn_mock_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();
    let project_root = TempDir::new().expect("project root");
    let project_skills = project_root.path().join(".belltower/skills");
    std::fs::create_dir_all(&project_skills).expect("project skills dir");
    std::fs::write(
        project_skills.join("20-project.md"),
        "Persist this project instruction in turn provenance.",
    )
    .expect("project instruction");

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: project_root.path().display().to_string(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("demo".to_owned()),
            objective: Some("answer".to_owned()),
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let send = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "hello"),
        })
        .send()
        .await
        .expect("send message");
    assert_eq!(send.status(), StatusCode::ACCEPTED);
    let send: SendMessageResponse = send.json().await.expect("send message body");
    assert_eq!(send.session_id, created.session.session_id);
    assert_eq!(send.branch_id, created.branch.branch_id);
    assert_eq!(send.outcome, SendMessageOutcome::Dispatched);
    settle(&api_client(&server), created.session.session_id).await;

    let messages: SessionMessagesResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/messages", created.session.session_id),
        )
        .send()
        .await
        .expect("messages request")
        .json()
        .await
        .expect("messages body");
    assert_eq!(messages.messages.len(), 2);
    assert!(messages.messages.iter().any(|message| {
        message.role == Role::Assistant
            && matches!(message.parts.as_slice(), [MessagePart::Text { text }] if text == "hello")
    }));

    let events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("events request")
        .json()
        .await
        .expect("events body");
    let turn_start = events
        .events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::TurnStarted { turn_id, .. } => Some((*turn_id, event.seq_id)),
            _ => None,
        })
        .expect("turn.started event");
    let turn_finish = events
        .events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::TurnFinished { turn_id, .. } => Some((*turn_id, event.seq_id)),
            _ => None,
        })
        .expect("turn.finished event");
    assert_eq!(turn_start.0, turn_finish.0);
    assert!(turn_finish.1 > turn_start.1);
    let turn_start_seq = turn_start.1.expect("turn.started seq");
    let instruction_provenance = events
        .events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::TurnInstructionProvenanceRecorded { provenance } => {
                Some((provenance, event.seq_id?))
            }
            _ => None,
        })
        .expect("turn instruction provenance event");
    assert_eq!(instruction_provenance.0.turn_id, turn_start.0);
    assert_eq!(instruction_provenance.0.provider, "openai-compatible");
    assert!(instruction_provenance.0.provider_overlay.is_some());
    assert!(
        instruction_provenance
            .0
            .instructions
            .iter()
            .any(|doc| doc.title == "20-project"
                && doc.body.contains("Persist this project instruction"))
    );
    assert!(
        instruction_provenance
            .0
            .rendered_system_prompt
            .contains("Objective:\nanswer")
    );
    assert!(
        instruction_provenance
            .0
            .rendered_system_prompt
            .contains("## 20-project")
    );
    assert!(
        instruction_provenance
            .0
            .rendered_system_prompt
            .contains("- `read`: inspect a known file")
    );
    assert!(
        instruction_provenance
            .0
            .rendered_system_prompt
            .contains("Runtime context:")
    );
    assert!(instruction_provenance.1 > turn_start_seq);
    let context_manifest = events
        .events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::TurnContextManifestRecorded { manifest } => {
                Some((manifest, event.seq_id?))
            }
            _ => None,
        })
        .expect("turn context manifest event");
    assert_eq!(context_manifest.0.turn_id, turn_start.0);
    assert_eq!(context_manifest.0.llm_call_ordinal, 1);
    assert_eq!(context_manifest.0.provider, "openai-compatible");
    assert_eq!(context_manifest.0.settings_revision_id, 1);
    assert!(context_manifest.0.system_prompt.present);
    assert!(!context_manifest.0.messages.is_empty());
    assert!(!context_manifest.0.tools.is_empty());
    assert!(context_manifest.1 > instruction_provenance.1);
    let completion_requested_seq = events
        .events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::CompletionRequested { .. } if event.turn_id == Some(turn_start.0) => {
                event.seq_id
            }
            _ => None,
        })
        .expect("completion.requested event seq");
    assert!(completion_requested_seq > context_manifest.1);
    assert!(
        events
            .events
            .iter()
            .any(|event| { matches!(event.payload, EventPayload::CompletionChunk { .. }) })
    );
    assert!(events.events.iter().any(|event| {
        matches!(event.payload, EventPayload::CompletionRequested { .. })
            && event.turn_id == Some(turn_start.0)
    }));
    assert!(events.events.iter().any(|event| {
        matches!(event.payload, EventPayload::CompletionFinished { .. })
            && event.turn_id == Some(turn_start.0)
    }));
    assert!(events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::CompletionFinished { usage, cost, .. }
            if usage.total_tokens == 19
                && usage.prompt_tokens == 12
                && usage.completion_tokens == 7
                && cost.as_ref().is_some_and(|value| value.total_usd > 0.0)
    )));
    let completion_raw_chunk_indexes = events
        .events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::CompletionChunk {
                raw_chunk_index, ..
            } if event.turn_id == Some(turn_start.0) => Some(raw_chunk_index),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!completion_raw_chunk_indexes.is_empty());
    assert!(
        completion_raw_chunk_indexes
            .iter()
            .all(|raw_chunk_index| raw_chunk_index.is_some())
    );

    let raw_chunks: SessionRawChunksResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/raw_chunks", created.session.session_id),
        )
        .send()
        .await
        .expect("raw chunks request")
        .json()
        .await
        .expect("raw chunks body");
    assert!(!raw_chunks.chunks.is_empty());
    assert!(raw_chunks.chunks.iter().all(|chunk| {
        chunk.branch_id == Some(created.branch.branch_id)
            && chunk.turn_id == Some(turn_start.0)
            && chunk.llm_call_ordinal == Some(1)
    }));
    let raw_chunk_event_ids = raw_chunks
        .chunks
        .iter()
        .filter_map(|chunk| chunk.event_id.as_deref())
        .collect::<std::collections::BTreeSet<_>>();
    let raw_chunk_events = events
        .events
        .iter()
        .filter(|event| matches!(event.payload, EventPayload::RawChunkPersisted { .. }))
        .map(|event| event.event_id.to_string())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        raw_chunk_event_ids
            .iter()
            .all(|event_id| raw_chunk_events.contains(*event_id))
    );
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&raw_chunks.chunks[0].content_base64)
        .expect("decode raw chunk");
    let decoded = String::from_utf8(decoded).expect("utf8 raw chunk");
    assert!(decoded.contains("\"choices\""));

    let turn_raw_chunks: TurnRawChunksPageResponse = server
        .request(
            &client,
            Method::GET,
            &format!(
                "sessions/{}/branches/{}/turns/{}/raw_chunks/page?limit=10",
                created.session.session_id, created.branch.branch_id, turn_start.0
            ),
        )
        .send()
        .await
        .expect("turn raw chunks request")
        .json()
        .await
        .expect("turn raw chunks body");
    assert_eq!(turn_raw_chunks.session_id, created.session.session_id);
    assert_eq!(turn_raw_chunks.branch_id, created.branch.branch_id);
    assert_eq!(turn_raw_chunks.turn_id, turn_start.0);
    assert_eq!(turn_raw_chunks.chunks.len(), raw_chunks.chunks.len());
    assert!(turn_raw_chunks.oldest_chunk_id.is_some());
    assert!(turn_raw_chunks.newest_chunk_id.is_some());
    assert!(!turn_raw_chunks.has_more_before);
    assert!(
        turn_raw_chunks
            .chunks
            .iter()
            .all(|chunk| chunk.event_id.is_some())
    );

    let call_scoped_raw_chunks: TurnRawChunksPageResponse = server
        .request(
            &client,
            Method::GET,
            &format!(
                "sessions/{}/branches/{}/turns/{}/raw_chunks/page?limit=10&llm_call_ordinal=1",
                created.session.session_id, created.branch.branch_id, turn_start.0
            ),
        )
        .send()
        .await
        .expect("call-scoped raw chunks request")
        .json()
        .await
        .expect("call-scoped raw chunks body");
    assert_eq!(
        call_scoped_raw_chunks.chunks.len(),
        turn_raw_chunks.chunks.len()
    );
    assert!(
        call_scoped_raw_chunks
            .chunks
            .iter()
            .all(|chunk| chunk.llm_call_ordinal == Some(1))
    );

    let branches: BranchesResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/branches", created.session.session_id),
        )
        .send()
        .await
        .expect("branches request")
        .json()
        .await
        .expect("branches body");
    assert_eq!(branches.branches.len(), 1);

    let branch_inspection: SessionBranchInspectionResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/branches/inspect", created.session.session_id),
        )
        .send()
        .await
        .expect("branch inspection request")
        .json()
        .await
        .expect("branch inspection body");
    assert_eq!(branch_inspection.branches.len(), 1);
    assert_eq!(
        branch_inspection.active_branch_id,
        Some(created.branch.branch_id)
    );
    assert!(branch_inspection.branches[0].is_active);

    let tree: SessionTreeResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/tree", created.session.session_id),
        )
        .send()
        .await
        .expect("tree request")
        .json()
        .await
        .expect("tree body");
    assert_eq!(
        tree.inspection.session.session_id,
        created.session.session_id
    );
    assert_eq!(tree.inspection.branches.len(), 1);
    assert!(tree.inspection.related_sessions.is_empty());

    let inspection: SessionInspectionResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}", created.session.session_id),
        )
        .send()
        .await
        .expect("inspection request")
        .json()
        .await
        .expect("inspection body");
    assert_eq!(
        inspection.inspection.session.session_id,
        created.session.session_id
    );
    assert_eq!(
        inspection
            .inspection
            .active_branch
            .map(|branch| branch.branch_id),
        Some(created.branch.branch_id)
    );
    assert_eq!(inspection.inspection.turn_count, 1);
    assert_eq!(inspection.inspection.message_count, 2);
    assert_eq!(
        inspection.inspection.raw_chunk_count,
        raw_chunks.chunks.len() as u32
    );
    assert!(matches!(
        inspection.inspection.runtime_state,
        bt_core::SessionRuntimeState::Idle
    ));
    assert!(inspection.inspection.related_sessions.is_empty());
    let cost_summary = inspection
        .inspection
        .cost_summary
        .as_ref()
        .expect("cost summary");
    assert_eq!(cost_summary.prompt_tokens, 12);
    assert_eq!(cost_summary.completion_tokens, 7);
    assert_eq!(cost_summary.total_tokens, 19);
    assert_eq!(cost_summary.unpriced_completion_count, 0);
    assert!((cost_summary.total_cost_usd - 0.000044).abs() < 1e-12);

    let turns: SessionTurnsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/turns", created.session.session_id),
        )
        .send()
        .await
        .expect("turns request")
        .json()
        .await
        .expect("turns body");
    assert_eq!(turns.turns.len(), 1);
    assert_eq!(turns.turns[0].message_count, 1);
    assert!(
        turns.turns[0]
            .status
            .as_deref()
            .is_some_and(|status| status == "completed")
    );

    let execution: SessionExecutionResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/execution", created.session.session_id),
        )
        .send()
        .await
        .expect("execution request")
        .json()
        .await
        .expect("execution body");
    assert_eq!(execution.inspection.session_id, created.session.session_id);
    assert_eq!(execution.inspection.turns.len(), 1);
    assert!(execution.inspection.event_counts.agent > 0);
    assert!(execution.inspection.event_counts.llm > 0);
    assert_eq!(
        execution.inspection.turns[0].source,
        Some(bt_core::TurnStartSource::UserMessage)
    );
    assert_eq!(execution.inspection.turns[0].resumed_from_call_id, None);
}

#[tokio::test]
async fn failed_streaming_turn_records_session_error_and_failed_turn_finish() {
    let mock_provider = spawn_mock_broken_stream_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("broken-stream".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let send = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "hello"),
        })
        .send()
        .await
        .expect("send message");
    assert_eq!(send.status(), StatusCode::ACCEPTED);
    let send: SendMessageResponse = send.json().await.expect("send message body");
    assert_eq!(send.outcome, SendMessageOutcome::Dispatched);

    let api = api_client(&server);
    settle(&api, created.session.session_id).await;
    let turns = api
        .session_turns(created.session.session_id)
        .await
        .expect("session turns");
    assert_eq!(
        turns.turns.last().and_then(|turn| turn.status.as_deref()),
        Some("failed")
    );

    let events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("events request")
        .json()
        .await
        .expect("events body");
    let turn_id = events
        .events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::TurnStarted { turn_id, .. } => Some(*turn_id),
            _ => None,
        })
        .expect("turn.started event");
    assert!(events.events.iter().any(|event| {
        matches!(event.payload, EventPayload::CompletionRequested { .. })
            && event.turn_id == Some(turn_id)
    }));
    assert!(events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::SessionError {
            class,
            code,
            retryable,
            ..
        } if *class == ErrorClass::Provider
            && code == "provider_error"
            && *retryable
            && event.turn_id == Some(turn_id)
    )));
    assert!(events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::TurnFinished {
            turn_id: finished_turn_id,
            status,
            finish_reason,
            ..
        } if *finished_turn_id == turn_id
            && status == "failed"
            && finish_reason.as_deref() == Some("provider_error")
    )));
    assert!(!events.events.iter().any(|event| {
        matches!(event.payload, EventPayload::CompletionFinished { .. })
            && event.turn_id == Some(turn_id)
    }));
    assert!(!events.events.iter().any(|event| {
        matches!(event.payload, EventPayload::RawChunkPersisted { .. })
            && event.turn_id == Some(turn_id)
    }));
}

#[tokio::test]
async fn pre_turn_failure_closes_prestarted_turn() {
    let mut config = BelltowerConfig::from_embedded().expect("config");
    config.connections.push(ConnectionDescriptor {
        id: ConnectionId::new("preflight-auth"),
        provider: "openai-chatgpt".to_owned(),
        base_url: reqwest::Url::parse("https://example.invalid/v1/").expect("preflight url"),
        default_model: "gpt-5.4-mini".to_owned(),
        auth_methods: Vec::new(),
        auth_sources: Vec::new(),
        model_fallbacks: Vec::new(),
        discoverable_model_selectors: Vec::new(),
    });
    let server = spawn_server(config).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("preflight-auth"),
            model_id: Some("gpt-5.4-mini".to_owned()),
            tool_mode: None,
            display_name: Some("pre-turn-failure".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let send = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "hello"),
        })
        .send()
        .await
        .expect("send message");
    assert_eq!(send.status(), StatusCode::ACCEPTED);
    let send: SendMessageResponse = send.json().await.expect("send message body");
    assert_eq!(send.outcome, SendMessageOutcome::Dispatched);

    let api = api_client(&server);
    settle(&api, created.session.session_id).await;
    let turns = api
        .session_turns(created.session.session_id)
        .await
        .expect("session turns");
    assert_eq!(
        turns.turns.last().and_then(|turn| turn.status.as_deref()),
        Some("failed")
    );

    let events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("events request")
        .json()
        .await
        .expect("events body");
    let started_index = events
        .events
        .iter()
        .position(|event| matches!(event.payload, EventPayload::TurnStarted { .. }))
        .expect("prestarted turn");
    let turn_id = events.events[started_index]
        .turn_id
        .expect("prestarted turn id");
    let error_index = events
        .events
        .iter()
        .position(|event| {
            matches!(
                &event.payload,
                EventPayload::SessionError {
                    class,
                    code,
                    ..
                } if *class == ErrorClass::Auth
                    && code == "auth_error"
                    && event.turn_id == Some(turn_id)
            )
        })
        .expect("auth session error");
    let finished_index = events
        .events
        .iter()
        .position(|event| {
            matches!(
                &event.payload,
                EventPayload::TurnFinished {
                    turn_id: finished_turn_id,
                    status,
                    finish_reason,
                    ..
                } if *finished_turn_id == turn_id
                    && status == "failed"
                    && finish_reason.as_deref() == Some("auth_error")
            )
        })
        .expect("failed turn finish");
    assert!(started_index < error_index);
    assert!(error_index < finished_index);
    assert_eq!(
        events
            .events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::TurnStarted { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn storage_failure_during_turn_records_error_and_halts() {
    let mock_provider = spawn_controlled_stream_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("storage-failure".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let send = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "hello"),
        })
        .send()
        .await
        .expect("send message");
    assert_eq!(send.status(), StatusCode::ACCEPTED);
    let send: SendMessageResponse = send.json().await.expect("send message body");
    assert_eq!(send.outcome, SendMessageOutcome::Dispatched);

    let mut request_seen = mock_provider.request_seen.clone();
    while !*request_seen.borrow_and_update() {
        request_seen
            .changed()
            .await
            .expect("provider request starts");
    }

    server
        .runtime
        .inject_next_store_append_error_for_test("injected store failure");
    mock_provider
        .release_stream
        .send(true)
        .expect("release provider stream");

    let api = api_client(&server);
    settle(&api, created.session.session_id).await;
    let turns = api
        .session_turns(created.session.session_id)
        .await
        .expect("session turns");
    assert_eq!(
        turns.turns.last().and_then(|turn| turn.status.as_deref()),
        Some("failed")
    );

    let events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("events request")
        .json()
        .await
        .expect("events body");
    let turn_id = events
        .events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::TurnStarted { turn_id, .. } => Some(*turn_id),
            _ => None,
        })
        .expect("turn.started event");
    assert!(events.events.iter().any(|event| {
        matches!(event.payload, EventPayload::CompletionRequested { .. })
            && event.turn_id == Some(turn_id)
    }));
    let session_errors = events
        .events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::SessionError {
                class,
                code,
                retryable,
                ..
            } => Some((event.turn_id, *class, code.as_str(), *retryable)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        session_errors
            .iter()
            .any(
                |(event_turn_id, class, code, retryable)| *class == ErrorClass::Storage
                    && *code == "storage_error"
                    && *retryable
                    && *event_turn_id == Some(turn_id)
            ),
        "session errors: {session_errors:?}"
    );
    assert!(events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::TurnFinished {
            turn_id: finished_turn_id,
            status,
            finish_reason,
            ..
        } if *finished_turn_id == turn_id
            && status == "failed"
            && finish_reason.as_deref() == Some("storage_error")
    )));
    assert!(!events.events.iter().any(|event| {
        matches!(event.payload, EventPayload::CompletionFinished { .. })
            && event.turn_id == Some(turn_id)
    }));
}

#[tokio::test]
async fn wall_clock_budget_exhaustion_records_checkpoint_and_persists_cancel_state() {
    let mock_provider = spawn_mock_interrupt_provider(1_100, &["hello"]).await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("budget-exhaust".to_owned()),
            objective: None,
            budget: Some(BudgetConfig {
                max_wall_clock_seconds: Some(1),
                max_tokens: None,
                max_turns: None,
                max_cost_usd: None,
            }),
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let send = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "hello"),
        })
        .send()
        .await
        .expect("send message");
    assert_eq!(send.status(), StatusCode::ACCEPTED);
    settle(&api_client(&server), created.session.session_id).await;

    let queue: SessionQueueResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/queue", created.session.session_id),
        )
        .send()
        .await
        .expect("queue request")
        .json()
        .await
        .expect("queue body");
    assert!(queue.inspection.cancel_requested);

    let inspection: SessionInspectionResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}", created.session.session_id),
        )
        .send()
        .await
        .expect("inspection request")
        .json()
        .await
        .expect("inspection body");
    let inspection_budget = inspection.inspection.budget.expect("budget inspection");
    assert!(inspection_budget.exhausted);
    assert!(inspection_budget.elapsed_seconds >= 1);

    let blocked = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "should remain blocked"),
        })
        .send()
        .await
        .expect("blocked send message");
    let blocked_error =
        assert_error_response(blocked, StatusCode::BAD_REQUEST, ErrorClass::Protocol).await;
    assert!(blocked_error.message.contains("budget exhausted"));

    let queue_after_block: SessionQueueResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/queue", created.session.session_id),
        )
        .send()
        .await
        .expect("queue after block request")
        .json()
        .await
        .expect("queue after block body");
    assert!(queue_after_block.inspection.cancel_requested);

    let budget = server
        .runtime
        .session_budget(created.session.session_id)
        .expect("budget lookup")
        .expect("budget projection");
    assert!(budget.elapsed_seconds >= 1);

    let events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("events request")
        .json()
        .await
        .expect("events body");
    assert!(events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::BudgetConfigured { budget }
            if budget.max_wall_clock_seconds == Some(1)
    )));
    assert!(events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::BudgetCheckpoint {
            elapsed_seconds,
            ..
        } if *elapsed_seconds >= 1
    )));
    assert!(events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::SessionCancelled { reason } if reason == "budget_exhausted"
    )));
}

#[tokio::test]
async fn wall_clock_budget_accounting_survives_server_restart() {
    let temp_dir = TempDir::new().expect("tempdir");
    let database_path = temp_dir.path().join("belltower.sqlite");

    let first_provider = spawn_mock_interrupt_provider(1_100, &["first"]).await;
    let first_server = spawn_server_with_database_path(
        config_for_mock_provider(&first_provider.base_url),
        &database_path,
        None,
    )
    .await;
    let client = Client::new();

    let created: CreateSessionResponse = first_server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("budget-restart".to_owned()),
            objective: None,
            budget: Some(BudgetConfig {
                max_wall_clock_seconds: Some(3),
                max_tokens: None,
                max_turns: None,
                max_cost_usd: None,
            }),
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let first_send = first_server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "first"),
        })
        .send()
        .await
        .expect("first send");
    assert_eq!(first_send.status(), StatusCode::ACCEPTED);
    settle(&api_client(&first_server), created.session.session_id).await;
    let first_budget = first_server
        .runtime
        .session_budget(created.session.session_id)
        .expect("first budget lookup")
        .expect("first budget projection");
    assert!(first_budget.elapsed_seconds >= 1);
    drop(first_server);

    let second_provider = spawn_mock_interrupt_provider(1_100, &["second"]).await;
    let second_server = spawn_server_with_database_path(
        config_for_mock_provider(&second_provider.base_url),
        &database_path,
        None,
    )
    .await;

    let second_send = second_server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "second"),
        })
        .send()
        .await
        .expect("second send");
    assert_eq!(second_send.status(), StatusCode::ACCEPTED);
    settle(&api_client(&second_server), created.session.session_id).await;

    let queue: SessionQueueResponse = second_server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/queue", created.session.session_id),
        )
        .send()
        .await
        .expect("queue request")
        .json()
        .await
        .expect("queue body");
    assert!(queue.inspection.cancel_requested);

    let second_budget = second_server
        .runtime
        .session_budget(created.session.session_id)
        .expect("second budget lookup")
        .expect("second budget projection");
    assert!(
        second_budget.elapsed_seconds > first_budget.elapsed_seconds,
        "budget elapsed should accumulate across restart"
    );

    let events: SessionEventsResponse = second_server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("events request")
        .json()
        .await
        .expect("events body");
    let checkpoints = events
        .events
        .iter()
        .filter(|event| matches!(event.payload, EventPayload::BudgetCheckpoint { .. }))
        .count();
    assert!(checkpoints >= 2);
    assert!(events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::SessionCancelled { reason } if reason == "budget_exhausted"
    )));
}

#[tokio::test]
async fn session_settings_can_be_updated() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("settings".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let updated: CreateSessionResponse = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}", created.session.session_id),
        )
        .json(&UpdateSessionRequest {
            connection_id: Some(ConnectionId::new("openai")),
            model_id: Some("o4-mini".to_owned()),
            tool_mode: None,
            reset_model_to_default: false,
        })
        .send()
        .await
        .expect("update session")
        .json()
        .await
        .expect("update session body");

    assert_eq!(updated.session.connection_id, ConnectionId::new("openai"));
    assert_eq!(updated.session.model_id.as_deref(), Some("o4-mini"));
}

#[tokio::test]
async fn session_budget_route_configures_runtime_budget() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("budget-route".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let response: SessionInspectionResponse = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/budget", created.session.session_id),
        )
        .json(&UpdateSessionBudgetRequest {
            budget: BudgetConfig {
                max_wall_clock_seconds: Some(30),
                max_tokens: Some(1_000),
                max_turns: Some(3),
                max_cost_usd: None,
            },
        })
        .send()
        .await
        .expect("update budget")
        .json()
        .await
        .expect("update budget body");

    let budget = response.inspection.budget.expect("budget inspection");
    assert_eq!(budget.budget.max_wall_clock_seconds, Some(30));
    assert_eq!(budget.budget.max_tokens, Some(1_000));
    assert_eq!(budget.budget.max_turns, Some(3));
    assert_eq!(budget.budget.max_cost_usd, None);
    assert!(!budget.exhausted);

    let events = server
        .runtime
        .all_events(created.session.session_id)
        .expect("events after budget update");
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::BudgetConfigured { budget }
            if budget.max_turns == Some(3)
                && budget.max_tokens == Some(1_000)
                && budget.max_wall_clock_seconds == Some(30)
    )));
}

#[tokio::test]
async fn compact_route_runs_runtime_owned_compaction() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("openai"),
            model_id: Some("o4-mini".to_owned()),
            tool_mode: None,
            display_name: Some("compact-demo".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    server
        .runtime
        .append_message(
            &created.session,
            &created.branch,
            Role::User,
            "first task context",
        )
        .expect("first message");
    server
        .runtime
        .append_message(
            &created.session,
            &created.branch,
            Role::Assistant,
            "acknowledged",
        )
        .expect("assistant message");
    server
        .runtime
        .append_message(
            &created.session,
            &created.branch,
            Role::User,
            "latest request",
        )
        .expect("latest message");

    let response: CompactSessionResponse = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/compact", created.session.session_id),
        )
        .json(&CompactSessionRequest {
            branch_id: created.branch.branch_id,
        })
        .send()
        .await
        .expect("compact request")
        .json()
        .await
        .expect("compact response");

    assert_eq!(response.session_id, created.session.session_id);
    assert_eq!(response.branch_id, created.branch.branch_id);
    assert_eq!(response.model_id, "o4-mini");
    let compaction = response.compaction.expect("compaction dto");
    assert_eq!(compaction.trigger, ContextCompactionTrigger::Forced);
    assert_eq!(compaction.phase, ContextCompactionPhase::Manual);
    assert_eq!(compaction.status, ContextCompactionStatus::Completed);
    assert_eq!(compaction.provider.as_deref(), Some("openai-compatible"));
    assert_eq!(compaction.model.as_deref(), Some("o4-mini"));
    assert!(compaction.context_boundary_seq_id.is_some());
    assert!(compaction.summary_message_id.is_some());
    assert!(compaction.first_kept_message_id.is_some());
    assert_eq!(
        compaction.first_kept_branch_id,
        Some(created.branch.branch_id)
    );
    assert!(compaction.first_kept_seq_id.is_some());
    assert!(
        compaction
            .summary
            .contains("Earlier conversation compacted")
    );

    let events = server
        .runtime
        .all_events(created.session.session_id)
        .expect("events after compaction");
    let event = events
        .iter()
        .find(|event| matches!(event.payload, EventPayload::ContextCompacted { .. }))
        .expect("context compacted event");
    match &event.payload {
        EventPayload::ContextCompacted {
            compaction_id,
            trigger,
            phase,
            status,
            summary_message_id,
            first_kept_message_id,
            first_kept_branch_id,
            first_kept_seq_id,
            summary,
            ..
        } => {
            assert_eq!(*compaction_id, compaction.compaction_id);
            assert_eq!(*trigger, ContextCompactionTrigger::Forced);
            assert_eq!(*phase, ContextCompactionPhase::Manual);
            assert_eq!(*status, ContextCompactionStatus::Completed);
            assert_eq!(*summary_message_id, compaction.summary_message_id);
            assert_eq!(*first_kept_message_id, compaction.first_kept_message_id);
            assert_eq!(*first_kept_branch_id, Some(created.branch.branch_id));
            assert_eq!(*first_kept_seq_id, compaction.first_kept_seq_id);
            assert!(summary.contains("Earlier conversation compacted"));
        }
        _ => unreachable!("matched context compacted event"),
    }
}

#[tokio::test]
async fn degraded_mcp_servers_do_not_break_turn_execution() {
    let mock_provider = spawn_mock_provider().await;
    let mut config = config_for_mock_provider(&mock_provider.base_url);
    config.mcp.servers = vec![McpServerConfig {
        name: "broken".to_owned(),
        enabled: true,
        transport: McpTransportConfig::Stdio {
            command: "/bin/false".to_owned(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
        },
    }];
    let server = spawn_server(config).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("mcp-degraded".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let send = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "hello"),
        })
        .send()
        .await
        .expect("send message");
    assert_eq!(send.status(), StatusCode::ACCEPTED);
    settle(&api_client(&server), created.session.session_id).await;

    let messages: SessionMessagesResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/messages", created.session.session_id),
        )
        .send()
        .await
        .expect("messages request")
        .json()
        .await
        .expect("messages body");
    assert!(messages.messages.iter().any(|message| {
        message.role == Role::Assistant
            && matches!(message.parts.as_slice(), [MessagePart::Text { text }] if text == "hello")
    }));

    let listed: McpServersResponse = server
        .request(&client, Method::GET, "mcp/servers")
        .send()
        .await
        .expect("mcp servers")
        .json()
        .await
        .expect("mcp servers body");
    assert!(matches!(
        listed.servers[0].status,
        McpServerStatus::Degraded { .. }
    ));
}

#[tokio::test]
async fn session_export_returns_legacy_bundle_jsonl_html_sharegpt_and_otlp() {
    let mock_provider = spawn_mock_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("export-demo".to_owned()),
            objective: Some("ship it".to_owned()),
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "hello"),
        })
        .send()
        .await
        .expect("send message");
    settle(&api_client(&server), created.session.session_id).await;

    let source_events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("events request")
        .json()
        .await
        .expect("events body");
    let source_event_ids = source_events
        .events
        .iter()
        .map(|event| event.event_id)
        .collect::<Vec<_>>();
    assert!(!source_event_ids.is_empty());

    let bundle: SessionExportResponse = server
        .request(
            &client,
            Method::GET,
            &format!(
                "sessions/{}/export/legacy-bundle",
                created.session.session_id
            ),
        )
        .send()
        .await
        .expect("bundle request")
        .json()
        .await
        .expect("bundle body");
    assert_eq!(bundle.format, ExportFormat::LegacyBundle);
    assert_eq!(bundle.content_type, "application/json");
    let bundle_document: Value = serde_json::from_str(&bundle.content).expect("valid bundle json");
    assert_eq!(bundle_document["schema_version"], json!(1));
    assert_eq!(
        bundle_document["session"]["session_id"],
        json!(created.session.session_id.to_string())
    );
    assert!(
        bundle_document["branches"]
            .as_array()
            .is_some_and(|branches| !branches.is_empty())
    );
    assert!(
        bundle_document["messages"]
            .as_array()
            .is_some_and(|messages| {
                !messages.is_empty()
                    && serde_json::to_string(messages)
                        .is_ok_and(|serialized| serialized.contains("hello"))
            })
    );
    assert!(
        bundle_document["events"]
            .as_array()
            .is_some_and(|events| events.len() == source_events.events.len())
    );
    let bundle_event_ids = bundle_document["events"]
        .as_array()
        .expect("bundle events")
        .iter()
        .map(|event| serde_json::from_value::<EventEnvelope>(event.clone()))
        .collect::<std::result::Result<Vec<_>, _>>()
        .expect("bundle event envelopes")
        .into_iter()
        .map(|event| event.event_id)
        .collect::<Vec<_>>();
    assert_eq!(bundle_event_ids, source_event_ids);
    let bundle_raw_chunk_ids = bundle_document["raw_chunks"]
        .as_array()
        .expect("bundle raw chunks")
        .iter()
        .filter_map(|chunk| chunk["chunk_id"].as_i64())
        .collect::<Vec<_>>();
    assert!(!bundle_raw_chunk_ids.is_empty());
    let referenced_raw_chunk_ids = source_events
        .events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::CompletionChunk {
                raw_chunk_index: Some(raw_chunk_index),
                ..
            } => Some(*raw_chunk_index),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!referenced_raw_chunk_ids.is_empty());
    assert!(
        referenced_raw_chunk_ids
            .iter()
            .all(|chunk_id| bundle_raw_chunk_ids.contains(chunk_id))
    );

    let jsonl: SessionExportResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/export/jsonl", created.session.session_id),
        )
        .send()
        .await
        .expect("jsonl request")
        .json()
        .await
        .expect("jsonl body");
    assert_eq!(jsonl.format, ExportFormat::Jsonl);
    assert!(jsonl.content.contains("\"event_id\""));
    let jsonl_events = jsonl
        .content
        .lines()
        .map(serde_json::from_str::<EventEnvelope>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .expect("jsonl event envelopes");
    assert_eq!(
        jsonl_events
            .iter()
            .map(|event| event.event_id)
            .collect::<Vec<_>>(),
        source_event_ids
    );
    assert!(
        jsonl_events
            .windows(2)
            .all(|pair| pair[0].seq_id < pair[1].seq_id)
    );

    let html: SessionExportResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/export/html", created.session.session_id),
        )
        .send()
        .await
        .expect("html request")
        .json()
        .await
        .expect("html body");
    assert_eq!(html.format, ExportFormat::Html);
    assert!(html.content.contains("<!doctype html>"));
    assert!(html.content.contains("hello"));
    assert!(html.content.contains("<html lang=\"en\">"));
    assert!(html.content.contains("</html>"));
    assert_eq!(
        html.content.matches("<section class=\"message\">").count(),
        2
    );

    let sharegpt: SessionExportResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/export/sharegpt", created.session.session_id),
        )
        .send()
        .await
        .expect("sharegpt request")
        .json()
        .await
        .expect("sharegpt body");
    assert_eq!(sharegpt.format, ExportFormat::ShareGpt);
    assert!(sharegpt.content.contains("\"conversations\""));
    assert!(sharegpt.content.contains("\"human\""));
    assert!(sharegpt.content.contains("\"gpt\""));
    // ShareGPT is intentionally lossy: approval state, raw chunks,
    // budget/control projections, and lineage stay in bundle/JSONL.
    // Pin the loss surface so future exporter changes are explicit.
    let sharegpt_document: Value = serde_json::from_str(&sharegpt.content).expect("sharegpt json");
    assert!(sharegpt_document.get("raw_chunks").is_none());
    assert!(sharegpt_document.get("events").is_none());
    let conversations = sharegpt_document["conversations"]
        .as_array()
        .expect("sharegpt conversations");
    assert_eq!(conversations.len(), 2);
    assert!(conversations.iter().any(|message| {
        message["from"] == "human"
            && message["value"]
                .as_str()
                .is_some_and(|value| value == "hello")
    }));
    assert!(conversations.iter().any(|message| {
        message["from"] == "gpt"
            && message["value"]
                .as_str()
                .is_some_and(|value| value == "hello")
    }));

    let otlp: SessionExportResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/export/otlp", created.session.session_id),
        )
        .send()
        .await
        .expect("otlp request")
        .json()
        .await
        .expect("otlp body");
    assert_eq!(otlp.format, ExportFormat::Otlp);
    assert_eq!(otlp.content_type, "application/json");
    assert!(otlp.content.contains("\"resourceSpans\""));
    assert!(otlp.content.contains("\"gen_ai.request.model\""));
}

#[tokio::test]
async fn otlp_push_uploads_real_protobuf_to_collector() {
    let mock_provider = spawn_mock_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let collector = spawn_mock_otlp_collector().await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("otlp-push-demo".to_owned()),
            objective: Some("validate phoenix upload".to_owned()),
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "hello"),
        })
        .send()
        .await
        .expect("send message");
    settle(&api_client(&server), created.session.session_id).await;

    let pushed: PushOtlpExportResponse = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/export/otlp/push", created.session.session_id),
        )
        .json(&PushOtlpExportRequest {
            endpoint: Some(collector.base_url.clone()),
            project_name: Some("belltower-test".to_owned()),
            api_key: Some("secret-token".to_owned()),
            headers: BTreeMap::new(),
        })
        .send()
        .await
        .expect("otlp push request")
        .json()
        .await
        .expect("otlp push body");

    assert_eq!(pushed.content_type, "application/x-protobuf");
    assert_eq!(pushed.status_code, 200);
    assert!(pushed.request_url.ends_with("/v1/traces"));

    let captured = collector
        .captured
        .lock()
        .expect("collector lock")
        .clone()
        .expect("collector request");
    assert_eq!(
        captured.headers.get("content-type"),
        Some(&"application/x-protobuf".to_owned())
    );
    assert_eq!(
        captured.headers.get("authorization"),
        Some(&"Bearer secret-token".to_owned())
    );
    assert_eq!(
        captured.headers.get("api_key"),
        Some(&"secret-token".to_owned())
    );

    let decoded =
        ExportTraceServiceRequest::decode(captured.body.as_slice()).expect("valid otlp protobuf");
    let resource = decoded.resource_spans[0]
        .resource
        .as_ref()
        .expect("resource");
    let project_attr = resource
        .attributes
        .iter()
        .find(|attr| attr.key == "openinference.project.name")
        .and_then(|attr| attr.value.as_ref())
        .and_then(|value| value.value.as_ref())
        .and_then(|value| match value {
            opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(value) => {
                Some(value.as_str())
            }
            _ => None,
        });
    assert_eq!(project_attr, Some("belltower-test"));
    assert!(!decoded.resource_spans[0].scope_spans[0].spans.is_empty());
}

#[tokio::test]
async fn otlp_push_reports_protobuf_partial_success_details() {
    let mock_provider = spawn_mock_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let collector =
        spawn_mock_otlp_collector_with_response(MockOtlpCollectorResponse::protobuf_ok(
            ExportTraceServiceResponse {
                partial_success: Some(ExportTracePartialSuccess {
                    rejected_spans: 3,
                    error_message: "collector rejected spans with invalid attributes".to_owned(),
                }),
            }
            .encode_to_vec(),
        ))
        .await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("otlp-protobuf-partial".to_owned()),
            objective: Some("validate collector partial success".to_owned()),
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let pushed: PushOtlpExportResponse = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/export/otlp/push", created.session.session_id),
        )
        .json(&PushOtlpExportRequest {
            endpoint: Some(collector.base_url.clone()),
            project_name: Some("belltower-test".to_owned()),
            api_key: None,
            headers: BTreeMap::new(),
        })
        .send()
        .await
        .expect("otlp push request")
        .json()
        .await
        .expect("otlp push body");

    assert_eq!(pushed.status_code, 200);
    assert_eq!(pushed.rejected_spans, Some(3));
    assert_eq!(
        pushed.warning.as_deref(),
        Some("collector rejected spans with invalid attributes")
    );
}

#[tokio::test]
async fn otlp_push_reports_json_warning_only_partial_success() {
    let mock_provider = spawn_mock_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let collector = spawn_mock_otlp_collector_with_response(MockOtlpCollectorResponse::json_ok(
        serde_json::to_vec(&ExportTraceServiceResponse {
            partial_success: Some(ExportTracePartialSuccess {
                rejected_spans: 0,
                error_message: "accepted with collector warning".to_owned(),
            }),
        })
        .expect("serialize response"),
    ))
    .await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("otlp-json-warning".to_owned()),
            objective: Some("validate collector warning".to_owned()),
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let pushed: PushOtlpExportResponse = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/export/otlp/push", created.session.session_id),
        )
        .json(&PushOtlpExportRequest {
            endpoint: Some(collector.base_url.clone()),
            project_name: None,
            api_key: None,
            headers: BTreeMap::new(),
        })
        .send()
        .await
        .expect("otlp push request")
        .json()
        .await
        .expect("otlp push body");

    assert_eq!(pushed.status_code, 200);
    assert_eq!(pushed.rejected_spans, None);
    assert_eq!(
        pushed.warning.as_deref(),
        Some("accepted with collector warning")
    );
}

#[tokio::test]
async fn otlp_push_keeps_success_when_collector_returns_unreadable_success_body() {
    let mock_provider = spawn_mock_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let collector = spawn_mock_otlp_collector_with_response(MockOtlpCollectorResponse {
        status: StatusCode::OK,
        content_type: Some("text/plain".to_owned()),
        body: b"collector accepted but returned plain text".to_vec(),
    })
    .await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("otlp-unreadable-success".to_owned()),
            objective: Some("validate collector parse fallback".to_owned()),
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let pushed: PushOtlpExportResponse = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/export/otlp/push", created.session.session_id),
        )
        .json(&PushOtlpExportRequest {
            endpoint: Some(collector.base_url.clone()),
            project_name: None,
            api_key: None,
            headers: BTreeMap::new(),
        })
        .send()
        .await
        .expect("otlp push request")
        .json()
        .await
        .expect("otlp push body");

    assert_eq!(pushed.status_code, 200);
    assert_eq!(pushed.rejected_spans, None);
    let warning = pushed.warning.expect("warning");
    assert!(warning.contains("unreadable success response"));
    assert!(warning.contains("text/plain"));
    assert!(warning.contains("collector accepted but returned plain text"));
}

#[tokio::test]
#[ignore = "requires live Arize/Phoenix credentials and network access"]
async fn live_otlp_push_to_arize_preserves_session_correlation_across_turn_traces() {
    let endpoint = std::env::var("PHOENIX_COLLECTOR_ENDPOINT")
        .or_else(|_| std::env::var("ARIZE_OTLP_ENDPOINT"))
        .unwrap_or_else(|_| "https://otlp.arize.com/v1/traces".to_owned());
    let api_key = std::env::var("PHOENIX_API_KEY")
        .or_else(|_| std::env::var("ARIZE_API_KEY"))
        .expect("set PHOENIX_API_KEY or ARIZE_API_KEY");
    let space_id = std::env::var("ARIZE_SPACE_ID")
        .or_else(|_| std::env::var("PHOENIX_SPACE_ID"))
        .expect("set ARIZE_SPACE_ID or PHOENIX_SPACE_ID");

    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();
    let project_root = tempfile::TempDir::new().expect("project root");

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: project_root.path().display().to_string(),
            connection_id: ConnectionId::new("openai"),
            model_id: Some("o4-mini".to_owned()),
            tool_mode: None,
            display_name: Some("phoenix-turn-trace-cutover".to_owned()),
            objective: Some("validate one trace per turn with shared session.id".to_owned()),
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let runtime = &server.runtime;
    let session = created.session.clone();
    let branch = created.branch.clone();

    runtime
        .append_message(&session, &branch, Role::User, "Summarize the workspace.")
        .expect("append turn one user");
    let turn_one = TurnId::new();
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            turn_one,
            "openai".to_owned(),
            "o4-mini".to_owned(),
            1,
            session.settings_revision_id,
            bt_core::TurnStartSource::UserMessage,
            None,
        )
        .expect("turn one started");
    runtime
        .record_completion_requested(
            session.session_id,
            branch.branch_id,
            1,
            "openai".to_owned(),
            "o4-mini".to_owned(),
            1,
            turn_one,
        )
        .expect("turn one llm requested");
    runtime
        .record_completion_chunk(
            session.session_id,
            branch.branch_id,
            Some(1),
            vec![CompletionDelta::AppendText {
                text: "This repo contains a Rust workspace named Belltower.".to_owned(),
            }],
            None,
            Some(turn_one),
        )
        .expect("turn one chunk");
    runtime
        .append_raw_message(
            &session,
            &branch,
            Message::text(
                Role::Assistant,
                "This repo contains a Rust workspace named Belltower.",
            ),
            Some(turn_one),
        )
        .expect("turn one assistant message");
    runtime
        .record_completion_finished(
            session.session_id,
            branch.branch_id,
            CompletionSummary {
                provider: "openai".to_owned(),
                model: "o4-mini".to_owned(),
                finish_reason: FinishReason::Stop,
                usage: TokenUsage {
                    prompt_tokens: 42,
                    completion_tokens: 14,
                    total_tokens: 56,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                },
                cost: Some(CostBreakdown {
                    prompt_usd: 0.000042,
                    completion_usd: 0.000028,
                    total_usd: 0.00007,
                    cache_read_usd: None,
                    cache_write_usd: None,
                    reasoning_usd: None,
                }),
                latency_ms: 18,
            },
            1,
            turn_one,
        )
        .expect("turn one llm finished");
    runtime
        .record_turn_finished(
            session.session_id,
            branch.branch_id,
            turn_one,
            "openai".to_owned(),
            "o4-mini".to_owned(),
            "completed".to_owned(),
            Some("stop".to_owned()),
            18,
        )
        .expect("turn one finished");

    runtime
        .append_message(
            &session,
            &branch,
            Role::User,
            "List the workspace files and then read Cargo.toml.",
        )
        .expect("append turn two user");
    let turn_two = TurnId::new();
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            turn_two,
            "openai".to_owned(),
            "o4-mini".to_owned(),
            3,
            session.settings_revision_id,
            bt_core::TurnStartSource::UserMessage,
            None,
        )
        .expect("turn two started");

    let list_call_id = ToolCallId::new("call-list");
    runtime
        .record_completion_requested(
            session.session_id,
            branch.branch_id,
            1,
            "openai".to_owned(),
            "o4-mini".to_owned(),
            3,
            turn_two,
        )
        .expect("turn two first llm requested");
    runtime
        .record_completion_chunk(
            session.session_id,
            branch.branch_id,
            Some(1),
            vec![CompletionDelta::OpenToolCall {
                call_id: list_call_id.to_string(),
                tool_name: "list".to_owned(),
                arguments: Some(serde_json::json!({"path":"."})),
            }],
            None,
            Some(turn_two),
        )
        .expect("turn two first llm chunk");
    runtime
        .append_raw_message(
            &session,
            &branch,
            Message::from_part(
                Role::Assistant,
                MessagePart::ToolCall {
                    call: ToolCall {
                        tool_name: "list".to_owned(),
                        call_id: list_call_id.to_string(),
                        arguments: serde_json::json!({"path":"."}),
                    },
                },
            ),
            Some(turn_two),
        )
        .expect("turn two list tool call message");
    runtime
        .record_completion_finished(
            session.session_id,
            branch.branch_id,
            CompletionSummary {
                provider: "openai".to_owned(),
                model: "o4-mini".to_owned(),
                finish_reason: FinishReason::ToolUse,
                usage: TokenUsage {
                    prompt_tokens: 64,
                    completion_tokens: 11,
                    total_tokens: 75,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                },
                cost: Some(CostBreakdown {
                    prompt_usd: 0.000064,
                    completion_usd: 0.000022,
                    total_usd: 0.000086,
                    cache_read_usd: None,
                    cache_write_usd: None,
                    reasoning_usd: None,
                }),
                latency_ms: 21,
            },
            1,
            turn_two,
        )
        .expect("turn two first llm finished");
    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            list_call_id.clone(),
            "list".to_owned(),
            serde_json::json!({"path":"."}),
            Some(turn_two),
        )
        .expect("list tool requested");
    let list_result = ToolResultEnvelope {
        call_id: list_call_id.clone(),
        tool_name: "list".to_owned(),
        is_error: false,
        output: serde_json::json!({"entries":["Cargo.toml","crates","docs"]}),
        duration_ms: Some(4),
    };
    runtime
        .record_tool_execution(
            session.session_id,
            branch.branch_id,
            list_result.clone(),
            Some(turn_two),
        )
        .expect("list tool finished");
    runtime
        .append_raw_message(
            &session,
            &branch,
            Message::from_part(
                Role::Tool,
                MessagePart::ToolResult {
                    result: list_result.clone(),
                },
            ),
            Some(turn_two),
        )
        .expect("list tool result message");

    let read_call_id = ToolCallId::new("call-read");
    runtime
        .record_completion_requested(
            session.session_id,
            branch.branch_id,
            2,
            "openai".to_owned(),
            "o4-mini".to_owned(),
            5,
            turn_two,
        )
        .expect("turn two second llm requested");
    runtime
        .record_completion_chunk(
            session.session_id,
            branch.branch_id,
            Some(2),
            vec![CompletionDelta::OpenToolCall {
                call_id: read_call_id.to_string(),
                tool_name: "read".to_owned(),
                arguments: Some(serde_json::json!({"path":"Cargo.toml"})),
            }],
            None,
            Some(turn_two),
        )
        .expect("turn two second llm chunk");
    runtime
        .append_raw_message(
            &session,
            &branch,
            Message::from_part(
                Role::Assistant,
                MessagePart::ToolCall {
                    call: ToolCall {
                        tool_name: "read".to_owned(),
                        call_id: read_call_id.to_string(),
                        arguments: serde_json::json!({"path":"Cargo.toml"}),
                    },
                },
            ),
            Some(turn_two),
        )
        .expect("turn two read tool call message");
    runtime
        .record_completion_finished(
            session.session_id,
            branch.branch_id,
            CompletionSummary {
                provider: "openai".to_owned(),
                model: "o4-mini".to_owned(),
                finish_reason: FinishReason::ToolUse,
                usage: TokenUsage {
                    prompt_tokens: 73,
                    completion_tokens: 9,
                    total_tokens: 82,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                },
                cost: Some(CostBreakdown {
                    prompt_usd: 0.000073,
                    completion_usd: 0.000018,
                    total_usd: 0.000091,
                    cache_read_usd: None,
                    cache_write_usd: None,
                    reasoning_usd: None,
                }),
                latency_ms: 17,
            },
            2,
            turn_two,
        )
        .expect("turn two second llm finished");
    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            read_call_id.clone(),
            "read".to_owned(),
            serde_json::json!({"path":"Cargo.toml"}),
            Some(turn_two),
        )
        .expect("read tool requested");
    let read_result = ToolResultEnvelope {
        call_id: read_call_id.clone(),
        tool_name: "read".to_owned(),
        is_error: false,
        output: serde_json::json!({"content":"[workspace]\nmembers = [\"crates/*\"]"}),
        duration_ms: Some(6),
    };
    runtime
        .record_tool_execution(
            session.session_id,
            branch.branch_id,
            read_result.clone(),
            Some(turn_two),
        )
        .expect("read tool finished");
    runtime
        .append_raw_message(
            &session,
            &branch,
            Message::from_part(
                Role::Tool,
                MessagePart::ToolResult {
                    result: read_result.clone(),
                },
            ),
            Some(turn_two),
        )
        .expect("read tool result message");

    runtime
        .record_completion_requested(
            session.session_id,
            branch.branch_id,
            3,
            "openai".to_owned(),
            "o4-mini".to_owned(),
            7,
            turn_two,
        )
        .expect("turn two final llm requested");
    runtime
        .record_completion_chunk(
            session.session_id,
            branch.branch_id,
            Some(3),
            vec![CompletionDelta::AppendText {
                text: "The workspace root contains Cargo.toml, crates, and docs.".to_owned(),
            }],
            None,
            Some(turn_two),
        )
        .expect("turn two final llm chunk");
    runtime
        .append_raw_message(
            &session,
            &branch,
            Message::text(
                Role::Assistant,
                "The workspace root contains Cargo.toml, crates, and docs.",
            ),
            Some(turn_two),
        )
        .expect("turn two final assistant message");
    runtime
        .record_completion_finished(
            session.session_id,
            branch.branch_id,
            CompletionSummary {
                provider: "openai".to_owned(),
                model: "o4-mini".to_owned(),
                finish_reason: FinishReason::Stop,
                usage: TokenUsage {
                    prompt_tokens: 91,
                    completion_tokens: 15,
                    total_tokens: 106,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                },
                cost: Some(CostBreakdown {
                    prompt_usd: 0.000091,
                    completion_usd: 0.00003,
                    total_usd: 0.000121,
                    cache_read_usd: None,
                    cache_write_usd: None,
                    reasoning_usd: None,
                }),
                latency_ms: 23,
            },
            3,
            turn_two,
        )
        .expect("turn two final llm finished");
    runtime
        .record_turn_finished(
            session.session_id,
            branch.branch_id,
            turn_two,
            "openai".to_owned(),
            "o4-mini".to_owned(),
            "completed".to_owned(),
            Some("stop".to_owned()),
            23,
        )
        .expect("turn two finished");

    let otlp: SessionExportResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/export/otlp", session.session_id),
        )
        .send()
        .await
        .expect("otlp export request")
        .json()
        .await
        .expect("otlp export body");
    let document: Value = serde_json::from_str(&otlp.content).expect("valid otlp export");
    let spans = document["resourceSpans"][0]["scopeSpans"][0]["spans"]
        .as_array()
        .expect("spans array");
    let trace_ids = spans
        .iter()
        .filter_map(|span| span["traceId"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(trace_ids.len(), 2, "expected one exported trace per turn");
    assert_eq!(
        spans.iter().filter(|span| span["name"] == "turn").count(),
        2,
        "expected one turn root span per exported trace"
    );
    assert_eq!(
        spans
            .iter()
            .filter(|span| span["name"] == "llm_call")
            .count(),
        4,
        "expected one llm span per provider round-trip"
    );
    assert_eq!(
        spans
            .iter()
            .filter(|span| span["name"] == "tool_execution")
            .count(),
        2,
        "expected sibling tool spans inside the second turn trace"
    );

    let session_id = session.session_id.to_string();
    let session_id_attr = |span: &Value| {
        span["attributes"]
            .as_array()
            .and_then(|attrs| attrs.iter().find(|attr| attr["key"] == "session.id"))
            .and_then(|attr| attr["value"]["stringValue"].as_str())
            .map(str::to_owned)
    };
    assert!(
        spans
            .iter()
            .all(|span| session_id_attr(span) == Some(session_id.clone()))
    );

    let mut headers = BTreeMap::new();
    headers.insert("space_id".to_owned(), space_id);
    let pushed: PushOtlpExportResponse = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/export/otlp/push", session.session_id),
        )
        .json(&PushOtlpExportRequest {
            endpoint: Some(endpoint),
            project_name: Some("belltower-live-turn-traces".to_owned()),
            api_key: Some(api_key),
            headers,
        })
        .send()
        .await
        .expect("live otlp push request")
        .json()
        .await
        .expect("live otlp push body");

    assert_eq!(pushed.status_code, 200);
    assert_eq!(pushed.rejected_spans, None);
    assert_eq!(pushed.warning, None);
    eprintln!(
        "live arize validation ok session_id={} request_url={} bytes_sent={}",
        session.session_id, pushed.request_url, pushed.bytes_sent
    );
}

#[tokio::test]
async fn operator_commands_are_recorded_in_session_events() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("commands-demo".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let response = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/commands", created.session.session_id),
        )
        .json(&RecordOperatorCommandRequest {
            command_type: "slash_command".to_owned(),
            raw_input: "/help".to_owned(),
            output: "help output".to_owned(),
            success: true,
        })
        .send()
        .await
        .expect("record operator command");
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("events request")
        .json()
        .await
        .expect("events body");

    assert!(events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::OperatorCommandRecorded {
            command_type,
            raw_input,
            output,
            success,
        } if command_type == "slash_command"
            && raw_input == "/help"
            && output == "help output"
            && *success
    )));
}

#[tokio::test]
async fn clear_queue_route_drains_runtime_queue_and_records_drops() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("queue-clear-demo".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    server
        .runtime
        .queue_message(
            created.session.session_id,
            created.branch.branch_id,
            Message::text(Role::User, "first queued"),
        )
        .expect("queue first");
    server
        .runtime
        .queue_message(
            created.session.session_id,
            created.branch.branch_id,
            Message::text(Role::User, "second queued"),
        )
        .expect("queue second");

    let cleared: SessionQueueClearResponse = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/queue/clear", created.session.session_id),
        )
        .send()
        .await
        .expect("clear queue request")
        .json()
        .await
        .expect("clear queue body");
    assert_eq!(cleared.session_id, created.session.session_id);
    assert_eq!(cleared.cleared_messages.len(), 2);
    assert!(
        cleared
            .cleared_messages
            .iter()
            .any(|queued| queue_message_input(&queued.message) == "first queued")
    );
    assert!(
        cleared
            .cleared_messages
            .iter()
            .any(|queued| queue_message_input(&queued.message) == "second queued")
    );

    let queue: SessionQueueResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/queue", created.session.session_id),
        )
        .send()
        .await
        .expect("queue request")
        .json()
        .await
        .expect("queue body");
    assert!(queue.inspection.queued_messages.is_empty());

    let events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("events request")
        .json()
        .await
        .expect("events body");
    let dropped = events
        .events
        .iter()
        .filter(|event| {
            matches!(
                &event.payload,
                EventPayload::OperatorCommandRecorded {
                    command_type,
                    success,
                    ..
                } if command_type == "queue_drop" && *success
            )
        })
        .count();
    assert_eq!(dropped, 2);
}

#[tokio::test]
async fn spawn_route_creates_child_session_with_lineage_and_events() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("openai"),
            model_id: Some("o4-mini".to_owned()),
            tool_mode: None,
            display_name: Some("parent".to_owned()),
            objective: Some("root objective".to_owned()),
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let turn_id = TurnId::new();
    server
        .runtime
        .record_turn_started(
            created.session.session_id,
            created.branch.branch_id,
            turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            created.session.settings_revision_id,
            bt_core::TurnStartSource::UserMessage,
            None,
        )
        .expect("turn started");
    server
        .runtime
        .record_turn_finished(
            created.session.session_id,
            created.branch.branch_id,
            turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            "completed".to_owned(),
            Some("Stop".to_owned()),
            10,
        )
        .expect("turn finished");

    let spawned: SpawnSessionResponse = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/spawn", created.session.session_id),
        )
        .json(&SpawnSessionRequest {
            parent_branch_id: created.branch.branch_id,
            parent_turn_id: None,
            objective: "inspect recent tool approvals".to_owned(),
            display_name: Some("child".to_owned()),
            connection_id: None,
            model_id: None,
        })
        .send()
        .await
        .expect("spawn session")
        .json()
        .await
        .expect("spawn response body");

    assert_eq!(spawned.parent_session_id, created.session.session_id);
    assert_eq!(spawned.parent_branch_id, created.branch.branch_id);
    assert_eq!(spawned.parent_turn_id, Some(turn_id));
    assert_eq!(
        spawned.child_session.parent_session_id,
        Some(created.session.session_id)
    );
    assert_eq!(
        spawned.child_session.parent_branch_id,
        Some(created.branch.branch_id)
    );
    assert_eq!(spawned.child_session.parent_turn_id, Some(turn_id));

    let parent_events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("parent events request")
        .json()
        .await
        .expect("parent events body");
    assert!(parent_events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::SessionSpawnRequested {
            child_session_id,
            objective,
            ..
        } if *child_session_id == spawned.child_session.session_id
            && objective == "inspect recent tool approvals"
            && event.turn_id == Some(turn_id)
    )));
    assert!(parent_events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::SessionSpawned {
            child_session_id,
            child_branch_id,
            objective,
        } if *child_session_id == spawned.child_session.session_id
            && *child_branch_id == spawned.child_branch.branch_id
            && objective == "inspect recent tool approvals"
            && event.turn_id == Some(turn_id)
    )));

    let child_events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", spawned.child_session.session_id),
        )
        .send()
        .await
        .expect("child events request")
        .json()
        .await
        .expect("child events body");
    assert!(child_events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::SessionHandoffRecorded {
            parent_session_id,
            parent_branch_id,
            parent_turn_id,
            objective,
            ..
        } if *parent_session_id == created.session.session_id
            && *parent_branch_id == created.branch.branch_id
            && *parent_turn_id == Some(turn_id)
            && objective == "inspect recent tool approvals"
    )));
}

#[tokio::test]
async fn workflow_route_reports_parent_child_runtime_state() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("openai"),
            model_id: Some("o4-mini".to_owned()),
            tool_mode: None,
            display_name: Some("parent".to_owned()),
            objective: Some("root objective".to_owned()),
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    server
        .runtime
        .append_message(&created.session, &created.branch, Role::User, "hello")
        .expect("append parent message");
    server
        .runtime
        .record_turn_started(
            created.session.session_id,
            created.branch.branch_id,
            TurnId::new(),
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            created.session.settings_revision_id,
            bt_core::TurnStartSource::UserMessage,
            None,
        )
        .expect("turn started");

    let spawned: SpawnSessionResponse = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/spawn", created.session.session_id),
        )
        .json(&SpawnSessionRequest {
            parent_branch_id: created.branch.branch_id,
            parent_turn_id: None,
            objective: "inspect recent tool approvals".to_owned(),
            display_name: Some("child".to_owned()),
            connection_id: None,
            model_id: None,
        })
        .send()
        .await
        .expect("spawn session")
        .json()
        .await
        .expect("spawn response body");

    server
        .runtime
        .record_approval_requested(
            spawned.child_session.session_id,
            spawned.child_branch.branch_id,
            bt_core::ToolCallId::new("call-shell"),
            "shell".to_owned(),
            None,
        )
        .expect("approval requested");

    let workflow: SessionWorkflowResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/workflow", spawned.child_session.session_id),
        )
        .send()
        .await
        .expect("workflow request")
        .json()
        .await
        .expect("workflow body");

    assert_eq!(
        workflow.inspection.root_session_id,
        created.session.session_id
    );
    assert_eq!(workflow.inspection.node_count, 2);
    assert_eq!(workflow.inspection.runtime_counts.working, 1);
    assert_eq!(workflow.inspection.runtime_counts.waiting_on_approval, 1);
    assert_eq!(workflow.inspection.nodes[0].child_session_count, 1);
    assert!(workflow.inspection.nodes[1].is_focus);
}

#[tokio::test]
async fn session_events_report_true_tail_beyond_replay_window() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();

    let (session, branch) = server
        .runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("tail-demo".to_owned()),
            None,
        )
        .expect("create session");

    for index in 0..550 {
        server
            .runtime
            .record_operator_command(
                session.session_id,
                branch.branch_id,
                "slash_command".to_owned(),
                format!("/history {index}"),
                format!("history output {index}"),
                true,
            )
            .expect("record operator command");
    }

    let inspection = server
        .runtime
        .inspect_session(session.session_id)
        .expect("inspect session")
        .expect("session should exist");

    let events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", session.session_id),
        )
        .send()
        .await
        .expect("events request")
        .json()
        .await
        .expect("events body");

    assert_eq!(events.events.len(), 500);
    assert_eq!(events.last_seq_id, inspection.last_seq_id);
    assert!(
        events.last_seq_id.expect("true tail seq")
            > events
                .events
                .last()
                .and_then(|event| event.seq_id)
                .expect("replay window tail seq")
    );
}

#[tokio::test]
async fn branch_operator_commands_follow_branch_lineage_without_post_fork_parent_events() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();

    let (session, root_branch) = server
        .runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("lineage-demo".to_owned()),
            None,
        )
        .expect("create session");

    server
        .runtime
        .record_operator_command(
            session.session_id,
            root_branch.branch_id,
            "slash_command".to_owned(),
            "/session".to_owned(),
            "root-before".to_owned(),
            true,
        )
        .expect("record root-before");

    let child_branch = server
        .runtime
        .create_branch(session.session_id, root_branch.branch_id, None, false, None)
        .expect("create child branch");

    server
        .runtime
        .record_operator_command(
            session.session_id,
            root_branch.branch_id,
            "slash_command".to_owned(),
            "/history".to_owned(),
            "root-after".to_owned(),
            true,
        )
        .expect("record root-after");

    server
        .runtime
        .record_operator_command(
            session.session_id,
            child_branch.branch_id,
            "slash_command".to_owned(),
            "/execution".to_owned(),
            "child".to_owned(),
            true,
        )
        .expect("record child");

    let commands: BranchOperatorCommandsResponse = server
        .request(
            &client,
            Method::GET,
            &format!(
                "sessions/{}/branches/{}/commands",
                session.session_id, child_branch.branch_id
            ),
        )
        .send()
        .await
        .expect("commands request")
        .json()
        .await
        .expect("commands body");

    let outputs = commands
        .events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::OperatorCommandRecorded { output, .. } => Some(output.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(outputs, vec!["root-before", "child"]);
    assert_eq!(
        commands.last_seq_id,
        server
            .runtime
            .inspect_session(session.session_id)
            .expect("inspect")
            .and_then(|inspection| inspection.last_seq_id)
    );
}

#[tokio::test]
async fn branch_transcript_pages_return_latest_window_and_older_backfill() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();

    let (session, root_branch) = server
        .runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("paged-demo".to_owned()),
            None,
        )
        .expect("create session");

    for text in ["first", "second", "third"] {
        server
            .runtime
            .append_message(&session, &root_branch, Role::User, text)
            .expect("append parent message");
    }
    for input in ["/one", "/two", "/three"] {
        server
            .runtime
            .record_operator_command(
                session.session_id,
                root_branch.branch_id,
                "slash_command".to_owned(),
                input.to_owned(),
                format!("output {input}"),
                true,
            )
            .expect("record parent command");
    }

    let child = server
        .runtime
        .create_branch(
            session.session_id,
            root_branch.branch_id,
            None,
            false,
            Some("handoff".to_owned()),
        )
        .expect("create child branch");
    server
        .runtime
        .append_message(&session, &child, Role::Assistant, "child")
        .expect("append child message");
    server
        .runtime
        .record_operator_command(
            session.session_id,
            child.branch_id,
            "slash_command".to_owned(),
            "/child".to_owned(),
            "output /child".to_owned(),
            true,
        )
        .expect("record child command");

    let latest_messages: BranchMessagesPageResponse = server
        .request(
            &client,
            Method::GET,
            &format!(
                "sessions/{}/branches/{}/messages/page?limit=2",
                session.session_id, child.branch_id
            ),
        )
        .send()
        .await
        .expect("latest message page request")
        .json()
        .await
        .expect("latest message page body");
    assert!(latest_messages.has_more_before);
    assert_eq!(latest_messages.messages.len(), 2);
    assert!(matches!(
        latest_messages.messages[0].message.parts.as_slice(),
        [MessagePart::Text { text }] if text == "third"
    ));
    assert!(matches!(
        latest_messages.messages[1].message.parts.as_slice(),
        [MessagePart::Text { text }] if text == "child"
    ));

    let older_messages: BranchMessagesPageResponse = server
        .request(
            &client,
            Method::GET,
            &format!(
                "sessions/{}/branches/{}/messages/page?limit=10&before_seq={}",
                session.session_id,
                child.branch_id,
                latest_messages.oldest_seq_id.expect("oldest seq"),
            ),
        )
        .send()
        .await
        .expect("older message page request")
        .json()
        .await
        .expect("older message page body");
    assert!(!older_messages.has_more_before);
    assert_eq!(older_messages.messages[0].seq_id, 0);

    let latest_commands: BranchOperatorCommandsPageResponse = server
        .request(
            &client,
            Method::GET,
            &format!(
                "sessions/{}/branches/{}/commands/page?limit=2",
                session.session_id, child.branch_id
            ),
        )
        .send()
        .await
        .expect("latest command page request")
        .json()
        .await
        .expect("latest command page body");
    assert!(latest_commands.has_more_before);
    let latest_inputs = latest_commands
        .commands
        .iter()
        .map(|command| command.raw_input.as_str())
        .collect::<Vec<_>>();
    assert_eq!(latest_inputs, vec!["/three", "/child"]);

    let older_commands: BranchOperatorCommandsPageResponse = server
        .request(
            &client,
            Method::GET,
            &format!(
                "sessions/{}/branches/{}/commands/page?limit=10&before_seq={}",
                session.session_id,
                child.branch_id,
                latest_commands.oldest_seq_id.expect("oldest command seq"),
            ),
        )
        .send()
        .await
        .expect("older command page request")
        .json()
        .await
        .expect("older command page body");
    assert!(!older_commands.has_more_before);
    let older_inputs = older_commands
        .commands
        .iter()
        .map(|command| command.raw_input.as_str())
        .collect::<Vec<_>>();
    assert_eq!(older_inputs, vec!["/one", "/two"]);
}

#[tokio::test]
async fn operator_shell_commands_record_tool_and_command_events() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();
    let project_root = tempfile::TempDir::new().expect("project root");

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: project_root.path().display().to_string(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("operator-shell-demo".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let response = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/commands/shell", created.session.session_id),
        )
        .json(&RunShellCommandRequest {
            raw_input: "!printf ok".to_owned(),
            command: "printf ok".to_owned(),
            timeout_seconds: Some(5),
        })
        .send()
        .await
        .expect("run shell command");
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("events request")
        .json()
        .await
        .expect("events body");

    let operation_index = events
        .events
        .iter()
        .position(|event| matches!(event.payload, EventPayload::ToolOperationRecorded { .. }))
        .expect("operator tool operation");
    let request_index = events
        .events
        .iter()
        .position(|event| matches!(event.payload, EventPayload::ToolCallRequested { .. }))
        .expect("operator tool request");
    let terminal_index = events
        .events
        .iter()
        .position(|event| matches!(event.payload, EventPayload::ToolExecutionFinished { .. }))
        .expect("operator tool terminal");
    let command_index = events
        .events
        .iter()
        .position(|event| matches!(event.payload, EventPayload::OperatorCommandRecorded { .. }))
        .expect("operator command audit event");
    assert_eq!(request_index, operation_index + 1);
    assert_eq!(command_index, terminal_index + 1);
    assert!(!events.events.iter().any(|event| {
        matches!(
            &event.payload,
            EventPayload::MessageAppended { message }
                if message.tool_call().is_some() || message.tool_result().is_some()
        )
    }));

    assert!(events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolOperationRecorded {
            tool_name,
            operation,
            ..
        } if tool_name == "shell"
            && operation.initiator == ToolOperationInitiator::Human
            && operation.is_read_only == Some(false)
    )));
    assert!(events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolCallRequested { tool_name, arguments, .. }
            if tool_name == "shell"
                && arguments["command"] == "printf ok"
    )));
    assert!(events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolExecutionFinished { tool_name, result, .. }
            if tool_name == "shell"
                && result.output["stdout"] == "ok"
    )));
    assert!(events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::OperatorCommandRecorded {
            command_type,
            raw_input,
            output,
            success,
        } if command_type == "shell_command"
            && raw_input == "!printf ok"
            && output.contains("$ printf ok")
            && output.contains("stdout:\nok")
            && *success
    )));
}

#[tokio::test]
async fn operator_shell_request_admission_is_atomic_on_storage_failure() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();
    let project_root = tempfile::TempDir::new().expect("project root");
    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: project_root.path().display().to_string(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("operator-shell-atomicity".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    server
        .runtime
        .inject_next_store_append_error_for_test("operator admission failure");
    let response = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/commands/shell", created.session.session_id),
        )
        .json(&RunShellCommandRequest {
            raw_input: "!printf should-not-run".to_owned(),
            command: "printf should-not-run".to_owned(),
            timeout_seconds: Some(5),
        })
        .send()
        .await
        .expect("run shell command");
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let events = server
        .runtime
        .all_events(created.session.session_id)
        .expect("canonical events");
    assert!(!events.iter().any(|event| {
        matches!(
            &event.payload,
            EventPayload::ToolOperationRecorded { .. }
                | EventPayload::ToolCallRequested { .. }
                | EventPayload::ToolExecutionFinished { .. }
                | EventPayload::OperatorCommandRecorded { .. }
        )
    }));
}

#[tokio::test]
async fn operator_shell_route_recovers_terminal_batch_after_storage_failure() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();
    let project_root = tempfile::TempDir::new().expect("project root");
    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: project_root.path().display().to_string(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("operator-shell-terminal-recovery".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let request_client = client.clone();
    let request_url = server.url(&format!(
        "sessions/{}/commands/shell",
        created.session.session_id
    ));
    let token = server.token.clone();
    let request_task = tokio::spawn(async move {
        request_client
            .post(request_url)
            .bearer_auth(token)
            .header(PROTOCOL_HEADER, PROTOCOL_VERSION)
            .json(&RunShellCommandRequest {
                raw_input: "!sleep 1; printf ok".to_owned(),
                command: "sleep 1; printf ok".to_owned(),
                timeout_seconds: Some(5),
            })
            .send()
            .await
            .expect("run shell command")
    });

    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let events = server
                .runtime
                .all_events(created.session.session_id)
                .expect("canonical events");
            if events.iter().any(|event| {
                matches!(
                    &event.payload,
                    EventPayload::ToolCallRequested { tool_name, .. } if tool_name == "shell"
                )
            }) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("operator request should be committed before execution finishes");
    server
        .runtime
        .inject_next_store_append_error_for_test("operator terminal append failure");

    let response = request_task.await.expect("request task");
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let events = server
        .runtime
        .all_events(created.session.session_id)
        .expect("canonical events");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::ToolCallRequested { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::ToolExecutionFinished { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::OperatorCommandRecorded { .. }))
            .count(),
        1
    );
    assert!(events.iter().any(|event| {
        matches!(
            &event.payload,
            EventPayload::SessionError { code, .. } if code == "storage_error"
        )
    }));
    let call_id = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::ToolCallRequested { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .expect("operator call id");
    let inspection = server
        .runtime
        .inspect_tool_call(created.session.session_id, call_id)
        .expect("tool inspection")
        .expect("operator tool call");
    assert_eq!(inspection.execution_status.as_deref(), Some("completed"));
    assert_eq!(
        inspection
            .result
            .as_ref()
            .and_then(|result| result["output"]["stdout"].as_str()),
        Some("ok")
    );
}

#[tokio::test]
async fn branch_create_and_activate_preserve_handoff_context() {
    let mock_provider = spawn_mock_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("branch-demo".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "hello"),
        })
        .send()
        .await
        .expect("send message");
    settle(&api_client(&server), created.session.session_id).await;

    let child: CreateBranchResponse = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/branches", created.session.session_id),
        )
        .json(&CreateBranchRequest {
            from_branch_id: created.branch.branch_id,
            from_event_id: None,
            activate: false,
            carry_summary: true,
        })
        .send()
        .await
        .expect("create branch")
        .json()
        .await
        .expect("create branch body");

    let branch_messages: SessionMessagesResponse = server
        .request(
            &client,
            Method::GET,
            &format!(
                "sessions/{}/branches/{}/messages",
                created.session.session_id, child.branch.branch_id
            ),
        )
        .send()
        .await
        .expect("branch messages")
        .json()
        .await
        .expect("branch messages body");
    assert_eq!(branch_messages.branch_id, Some(child.branch.branch_id));
    assert!(matches!(
        branch_messages.messages[0].parts.as_slice(),
        [MessagePart::Text { text }] if text.contains("Earlier conversation compacted")
    ));
    assert!(branch_messages.messages.iter().any(|message| matches!(
        message.parts.as_slice(),
        [MessagePart::Text { text }] if text == "hello"
    )));

    let activate = server
        .request(
            &client,
            Method::POST,
            &format!(
                "sessions/{}/branches/{}/activate",
                created.session.session_id, child.branch.branch_id
            ),
        )
        .json(&ActivateBranchRequest {
            carry_summary: true,
        })
        .send()
        .await
        .expect("activate branch");
    assert_eq!(activate.status(), StatusCode::ACCEPTED);

    let default_messages: SessionMessagesResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/messages", created.session.session_id),
        )
        .send()
        .await
        .expect("default messages")
        .json()
        .await
        .expect("default messages body");
    assert_eq!(default_messages.branch_id, Some(child.branch.branch_id));
}

#[tokio::test]
async fn branch_create_can_fork_from_explicit_event_boundary() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("branch-from-event".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    server
        .runtime
        .append_message(&created.session, &created.branch, Role::User, "first")
        .expect("append first");
    server
        .runtime
        .append_message(&created.session, &created.branch, Role::Assistant, "second")
        .expect("append second");
    let fork_event_id = server
        .runtime
        .all_events(created.session.session_id)
        .expect("events")
        .into_iter()
        .find(|event| {
            matches!(
                &event.payload,
                EventPayload::MessageAppended { message }
                    if message.parts.iter().any(|part| {
                        matches!(part, MessagePart::Text { text } if text == "first")
                    })
            )
        })
        .expect("first message event")
        .event_id;

    let child: CreateBranchResponse = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/branches", created.session.session_id),
        )
        .json(&CreateBranchRequest {
            from_branch_id: created.branch.branch_id,
            from_event_id: Some(fork_event_id),
            activate: false,
            carry_summary: true,
        })
        .send()
        .await
        .expect("create branch")
        .json()
        .await
        .expect("create branch body");
    assert_eq!(child.branch.parent_event_id, Some(fork_event_id));

    server
        .runtime
        .append_message(&created.session, &child.branch, Role::User, "child")
        .expect("append child");
    let branch_messages: SessionMessagesResponse = server
        .request(
            &client,
            Method::GET,
            &format!(
                "sessions/{}/branches/{}/messages",
                created.session.session_id, child.branch.branch_id
            ),
        )
        .send()
        .await
        .expect("branch messages")
        .json()
        .await
        .expect("branch messages body");
    let texts = branch_messages
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            MessagePart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(texts.contains(&"first"));
    assert!(texts.contains(&"child"));
    assert!(!texts.contains(&"second"));
}

#[tokio::test]
async fn event_stream_replays_from_last_event_id_and_delivers_live_updates() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("stream".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create response");

    let seed_steer = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/steer", created.session.session_id),
        )
        .json(&SteerSessionRequest {
            message: "seed".to_owned(),
        })
        .send()
        .await
        .expect("seed steer");
    assert_eq!(seed_steer.status(), StatusCode::ACCEPTED);

    let initial_events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("initial events request")
        .json()
        .await
        .expect("initial events body");
    let start_seq = initial_events.last_seq_id.expect("seed event seq id");

    let replayed_steer = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/steer", created.session.session_id),
        )
        .json(&SteerSessionRequest {
            message: "replayed".to_owned(),
        })
        .send()
        .await
        .expect("replayed steer");
    assert_eq!(replayed_steer.status(), StatusCode::ACCEPTED);

    let response = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events/stream", created.session.session_id),
        )
        .header("Last-Event-ID", start_seq.to_string())
        .send()
        .await
        .expect("stream request");
    assert_eq!(response.status(), StatusCode::OK);
    let mut reader = SseReader::new(response);

    let replayed = reader.next().await;
    assert_eq!(replayed.protocol_version, PROTOCOL_VERSION);
    let replayed_event: EventEnvelope =
        serde_json::from_value(replayed.data).expect("replayed event payload");
    assert!(matches!(
        replayed_event.payload,
        EventPayload::SessionSteered { ref message, .. } if message == "replayed"
    ));

    let live_steer = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/steer", created.session.session_id),
        )
        .json(&SteerSessionRequest {
            message: "live".to_owned(),
        })
        .send()
        .await
        .expect("live steer");
    assert_eq!(live_steer.status(), StatusCode::ACCEPTED);

    let live = reader.next().await;
    let live_event: EventEnvelope = serde_json::from_value(live.data).expect("live event payload");
    assert!(matches!(
        live_event.payload,
        EventPayload::SessionSteered { ref message, .. } if message == "live"
    ));
}

#[test]
fn sse_error_payload_uses_raw_envelope_shape() {
    let data = serde_json::to_value(ErrorEnvelope {
        class: ErrorClass::Runtime,
        code: "replay_failed".to_owned(),
        message: "store unavailable".to_owned(),
        retryable: true,
        details: None,
    })
    .expect("error envelope value");
    let payload = raw_sse_payload(42, "error", data);
    let envelope: RawSseEnvelope = serde_json::from_str(&payload).expect("raw SSE envelope");

    assert_eq!(envelope.id, 42);
    assert_eq!(envelope.event, "error");
    assert_eq!(envelope.protocol_version, PROTOCOL_VERSION);
    assert_eq!(envelope.data["code"], "replay_failed");
    assert_eq!(envelope.data["retryable"], true);
}

#[tokio::test]
async fn event_stream_delivers_completion_chunks_before_turn_finishes() {
    let mock_provider = spawn_mock_live_stream_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("live-stream".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create response");

    let response = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events/stream", created.session.session_id),
        )
        .send()
        .await
        .expect("stream request");
    assert_eq!(response.status(), StatusCode::OK);
    let mut reader = SseReader::new(response);

    let send = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "hello"),
        })
        .send()
        .await
        .expect("send message request");
    assert_eq!(send.status(), StatusCode::ACCEPTED);

    let mut saw_completion_requested = false;
    let mut saw_completion_chunk = false;
    let mut saw_turn_finished_before_chunk = false;
    while !saw_completion_chunk {
        let envelope = tokio::time::timeout(std::time::Duration::from_secs(5), reader.next())
            .await
            .expect("stream should deliver events");
        let event: EventEnvelope = serde_json::from_value(envelope.data).expect("event payload");
        match event.payload {
            EventPayload::CompletionRequested { .. } => {
                saw_completion_requested = true;
            }
            EventPayload::CompletionChunk { deltas, .. } => {
                if deltas
                    .iter()
                    .any(|delta| matches!(delta, CompletionDelta::AppendText { .. }))
                {
                    saw_completion_chunk = true;
                }
            }
            EventPayload::TurnFinished { .. } => {
                saw_turn_finished_before_chunk = true;
            }
            _ => {}
        }
    }

    assert!(saw_completion_requested);
    assert!(
        !saw_turn_finished_before_chunk,
        "completion chunks should stream before the turn finishes"
    );
    // The dispatched turn must still be in flight when the first chunk is
    // observed: chunks are delivered live, not replayed after completion.
    let api = api_client(&server);
    let mid_turn_queue = api
        .session_queue(created.session.session_id)
        .await
        .expect("mid-turn queue inspection");
    assert_eq!(
        mid_turn_queue.inspection.runtime_state,
        SessionRuntimeState::Working,
        "turn should still be running when the first chunk is streamed"
    );

    let settled = settle(&api, created.session.session_id).await;
    assert_eq!(settled, SessionRuntimeState::Idle);
}

#[tokio::test]
async fn event_stream_recovers_from_broadcast_lag_by_replaying_store_events() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("lag-replay".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create response");

    server
        .runtime
        .record_operator_command(
            created.session.session_id,
            created.branch.branch_id,
            "seed".to_owned(),
            "/seed".to_owned(),
            "seed".to_owned(),
            true,
        )
        .expect("seed command");

    let initial_events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("initial events request")
        .json()
        .await
        .expect("initial events body");
    let start_seq = initial_events.last_seq_id.expect("seed event seq id");

    let response = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events/stream", created.session.session_id),
        )
        .header("Last-Event-ID", start_seq.to_string())
        .send()
        .await
        .expect("stream request");
    assert_eq!(response.status(), StatusCode::OK);
    let mut reader = SseReader::new(response);

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    for index in 0..1300 {
        server
            .runtime
            .record_operator_command(
                created.session.session_id,
                created.branch.branch_id,
                "burst".to_owned(),
                format!("/burst {index}"),
                format!("burst-{index}"),
                true,
            )
            .expect("record burst command");
    }

    let mut outputs = Vec::new();
    while outputs.len() < 1300 {
        let envelope = tokio::time::timeout(std::time::Duration::from_secs(5), reader.next())
            .await
            .expect("stream should keep replaying after lag");
        let event: EventEnvelope = serde_json::from_value(envelope.data).expect("event payload");
        if let EventPayload::OperatorCommandRecorded { output, .. } = event.payload
            && output.starts_with("burst-")
        {
            outputs.push(output);
        }
    }

    assert_eq!(outputs.len(), 1300);
    assert_eq!(outputs.first().map(String::as_str), Some("burst-0"));
    assert_eq!(outputs.last().map(String::as_str), Some("burst-1299"));
}

#[tokio::test]
async fn event_stream_replays_across_process_restart_without_duplicates() {
    let temp_dir = TempDir::new().expect("tempdir");
    let database_path = temp_dir.path().join("belltower.sqlite");
    let config = BelltowerConfig::from_embedded().expect("config");
    let first_server = spawn_server_with_database_path(config.clone(), &database_path, None).await;
    let client = Client::new();
    let api = api_client(&first_server);

    let created = api
        .create_session(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("restart-replay".to_owned()),
            objective: None,
            budget: None,
        })
        .await
        .expect("create session");

    let seeded = api
        .record_operator_command(
            created.session.session_id,
            &RecordOperatorCommandRequest {
                command_type: "slash_command".to_owned(),
                raw_input: "/seed".to_owned(),
                output: "restart-replay-seed".to_owned(),
                success: true,
            },
        )
        .await
        .expect("record seed command");
    assert_eq!(seeded.status(), StatusCode::ACCEPTED);

    let initial_events = api
        .session_events(created.session.session_id, None)
        .await
        .expect("initial events");
    let start_cursor = initial_events.last_seq_id.expect("initial seq id");

    let response = first_server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events/stream", created.session.session_id),
        )
        .header("Last-Event-ID", start_cursor.to_string())
        .send()
        .await
        .expect("first stream request");
    assert_eq!(response.status(), StatusCode::OK);
    let mut first_reader = SseReader::new(response);

    let first_recorded = api
        .record_operator_command(
            created.session.session_id,
            &RecordOperatorCommandRequest {
                command_type: "slash_command".to_owned(),
                raw_input: "/history".to_owned(),
                output: "restart-replay-a".to_owned(),
                success: true,
            },
        )
        .await
        .expect("record first command");
    assert_eq!(first_recorded.status(), StatusCode::ACCEPTED);

    let second_recorded = api
        .record_operator_command(
            created.session.session_id,
            &RecordOperatorCommandRequest {
                command_type: "slash_command".to_owned(),
                raw_input: "/status".to_owned(),
                output: "restart-replay-b".to_owned(),
                success: true,
            },
        )
        .await
        .expect("record second command");
    assert_eq!(second_recorded.status(), StatusCode::ACCEPTED);

    let replayed_a = tokio::time::timeout(std::time::Duration::from_secs(5), first_reader.next())
        .await
        .expect("first process should stream replay-a");
    let replayed_a_event: EventEnvelope =
        serde_json::from_value(replayed_a.data).expect("replay-a payload");
    assert!(matches!(
        replayed_a_event.payload,
        EventPayload::OperatorCommandRecorded { ref output, .. } if output == "restart-replay-a"
    ));

    let restart_cursor = replayed_a.id;

    let replayed_b_before_shutdown =
        tokio::time::timeout(std::time::Duration::from_secs(5), first_reader.next())
            .await
            .expect("first process should stream replay-b");
    let replayed_b_before_shutdown_event: EventEnvelope =
        serde_json::from_value(replayed_b_before_shutdown.data)
            .expect("replay-b before shutdown payload");
    assert!(matches!(
        replayed_b_before_shutdown_event.payload,
        EventPayload::OperatorCommandRecorded { ref output, .. } if output == "restart-replay-b"
    ));
    let pre_shutdown_tail = replayed_b_before_shutdown.id;
    assert!(pre_shutdown_tail > restart_cursor);

    drop(first_server);

    let second_server = spawn_server_with_database_path(config, &database_path, None).await;
    let restarted_api = api_client(&second_server);

    let restarted_response = second_server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events/stream", created.session.session_id),
        )
        .header("Last-Event-ID", restart_cursor.to_string())
        .send()
        .await
        .expect("restarted stream request");
    assert_eq!(restarted_response.status(), StatusCode::OK);
    let mut restarted_reader = SseReader::new(restarted_response);

    let replayed_after_restart =
        tokio::time::timeout(std::time::Duration::from_secs(5), restarted_reader.next())
            .await
            .expect("second process should replay replay-b");
    assert!(
        replayed_after_restart.id > restart_cursor,
        "replayed event must start strictly after the cursor"
    );
    assert_eq!(replayed_after_restart.id, pre_shutdown_tail);
    let replayed_after_restart_event: EventEnvelope =
        serde_json::from_value(replayed_after_restart.data)
            .expect("replay-b after restart payload");
    assert!(matches!(
        replayed_after_restart_event.payload,
        EventPayload::OperatorCommandRecorded { ref output, .. } if output == "restart-replay-b"
    ));

    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(150),
            restarted_reader.next(),
        )
        .await
        .is_err(),
        "no duplicate pre-shutdown events should replay after restart"
    );

    let third_recorded = restarted_api
        .record_operator_command(
            created.session.session_id,
            &RecordOperatorCommandRequest {
                command_type: "slash_command".to_owned(),
                raw_input: "/session".to_owned(),
                output: "restart-replay-c".to_owned(),
                success: true,
            },
        )
        .await
        .expect("record third command");
    assert_eq!(third_recorded.status(), StatusCode::ACCEPTED);

    let appended_after_restart =
        tokio::time::timeout(std::time::Duration::from_secs(5), restarted_reader.next())
            .await
            .expect("second process should stream replay-c");
    assert!(
        appended_after_restart.id > pre_shutdown_tail,
        "post-restart event ids must remain strictly monotonic"
    );
    let appended_after_restart_event: EventEnvelope =
        serde_json::from_value(appended_after_restart.data)
            .expect("replay-c after restart payload");
    assert!(matches!(
        appended_after_restart_event.payload,
        EventPayload::OperatorCommandRecorded { ref output, .. } if output == "restart-replay-c"
    ));
}

// Design note for 5.4a-min:
// Sequence: create a session, record one durable slash-command result through the HTTP API,
// start a model turn that pauses on a built-in shell approval, resume it, pause again on an
// MCP approval, resume it, then inspect/export the finished session.
// Inspection surfaces: session/queue/execution/tool-call/messages/raw-chunks/branch-commands
// plus bundle export and MCP inventory. The assertions intentionally avoid transcript
// heuristics and instead use the canonical read surfaces the TUI and clients consume.
// Principle-10 parity assertions: the built-in shell call and the MCP echo_remote call both
// appear as canonical stored events, both expose comparable SessionToolCallInspection shape,
// both appear in turn execution summaries, and both survive bundle export without
// source-specific reconstruction logic.
#[tokio::test]
async fn same_turn_continuation_manifest_uses_committed_tool_messages() {
    let mock_provider = spawn_mock_safe_tool_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = api_client(&server);
    let project_root = tempfile::TempDir::new().expect("project root");
    std::fs::write(project_root.path().join("README.md"), "fixture").expect("fixture file");
    let created = client
        .create_session(&CreateSessionRequest {
            approval_mode: None,
            project_root: project_root.path().display().to_string(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("same-turn-tool-context".to_owned()),
            objective: None,
            budget: None,
        })
        .await
        .expect("create session");

    let dispatched = client
        .send_message(
            created.session.session_id,
            &SendMessageRequest {
                branch_id: created.branch.branch_id,
                message: Message::text(Role::User, "list the workspace"),
            },
        )
        .await
        .expect("send message");
    assert!(matches!(dispatched.outcome, SendMessageOutcome::Dispatched));
    settle(&client, created.session.session_id).await;

    let events = client
        .session_events(created.session.session_id, None)
        .await
        .expect("events")
        .events;
    let call_id = ToolCallId::new("call-list-same-turn");
    let requested = events
        .iter()
        .find(|event| {
            matches!(
                &event.payload,
                EventPayload::ToolCallRequested {
                    call_id: event_call_id,
                    tool_name,
                    ..
                } if *event_call_id == call_id && tool_name == "list"
            )
        })
        .expect("tool request");
    let request_message = events
        .iter()
        .find(|event| {
            matches!(
                &event.payload,
                EventPayload::MessageAppended { message }
                    if message.tool_call().is_some_and(|call| call.call_id == call_id.to_string())
            )
        })
        .expect("tool-call message");
    let finished = events
        .iter()
        .find(|event| {
            matches!(
                &event.payload,
                EventPayload::ToolExecutionFinished {
                    call_id: event_call_id,
                    ..
                } if *event_call_id == call_id
            )
        })
        .expect("tool terminal");
    let result_message = events
        .iter()
        .find(|event| {
            matches!(
                &event.payload,
                EventPayload::MessageAppended { message }
                    if message.tool_result().is_some_and(|result| result.call_id == call_id)
            )
        })
        .expect("tool-result message");
    let continuation = events
        .iter()
        .find(|event| {
            event.seq_id > result_message.seq_id
                && matches!(event.payload, EventPayload::CompletionRequested { .. })
        })
        .expect("same-turn continuation");
    let manifest = events
        .iter()
        .find_map(|event| {
            if event.seq_id > result_message.seq_id
                && event.seq_id < continuation.seq_id
                && let EventPayload::TurnContextManifestRecorded { manifest } = &event.payload
            {
                Some(manifest)
            } else {
                None
            }
        })
        .expect("continuation manifest");
    let request_message_value = match &request_message.payload {
        EventPayload::MessageAppended { message } => message,
        _ => unreachable!("filtered request message"),
    };
    let result_message_value = match &result_message.payload {
        EventPayload::MessageAppended { message } => message,
        _ => unreachable!("filtered result message"),
    };

    assert_eq!(requested.turn_id, finished.turn_id);
    assert_eq!(finished.turn_id, continuation.turn_id);
    assert!(
        requested.seq_id < request_message.seq_id
            && request_message.seq_id < finished.seq_id
            && finished.seq_id < result_message.seq_id
            && result_message.seq_id < continuation.seq_id
    );
    assert!(manifest.messages.iter().any(|source| {
        source.message_id == request_message_value.message_id
            && source.source_seq_id == request_message.seq_id
    }));
    assert!(manifest.messages.iter().any(|source| {
        source.message_id == result_message_value.message_id
            && source.source_seq_id == result_message.seq_id
    }));
    assert!(
        !events.iter().any(|event| matches!(
            &event.payload,
            EventPayload::ToolApprovalRequested {
                call_id: event_call_id,
                ..
            } if *event_call_id == call_id
        )),
        "safe tool should continue in the same turn without approval"
    );

    let requests = mock_provider
        .requests
        .as_ref()
        .expect("captured requests")
        .lock()
        .expect("requests lock");
    assert_eq!(requests.len(), 2);
}

#[tokio::test]
async fn inspection_contract_min_reconstructs_canonical_session_truth() {
    let mock_provider = spawn_mock_inspection_contract_provider().await;
    let mcp_fixture = spawn_mock_http_mcp_server().await;
    let mut config = config_for_mock_provider(&mock_provider.base_url);
    config.mcp.servers = vec![
        McpServerConfig {
            name: "fixture".to_owned(),
            enabled: true,
            transport: McpTransportConfig::StreamableHttp {
                base_url: mcp_fixture.base_url.parse().expect("fixture mcp url"),
                headers: BTreeMap::new(),
            },
        },
        McpServerConfig {
            name: "broken".to_owned(),
            enabled: true,
            transport: McpTransportConfig::Stdio {
                command: "/bin/false".to_owned(),
                args: Vec::new(),
                cwd: None,
                env: BTreeMap::new(),
            },
        },
    ];
    let server = spawn_server(config).await;
    let client = api_client(&server);
    let project_root = tempfile::TempDir::new().expect("project root");

    let created = client
        .create_session(&CreateSessionRequest {
            approval_mode: None,
            project_root: project_root.path().display().to_string(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("inspection-contract".to_owned()),
            objective: Some("exercise canonical inspection truth".to_owned()),
            budget: None,
        })
        .await
        .expect("create session");

    let operator_status = client
        .record_operator_command(
            created.session.session_id,
            &RecordOperatorCommandRequest {
                command_type: "slash_command".to_owned(),
                raw_input: "/status".to_owned(),
                output: "status output".to_owned(),
                success: true,
            },
        )
        .await
        .expect("record operator command");
    assert_eq!(operator_status.status(), StatusCode::ACCEPTED);

    let dispatched = client
        .send_message(
            created.session.session_id,
            &SendMessageRequest {
                branch_id: created.branch.branch_id,
                message: Message::text(Role::User, "run the inspection contract demo"),
            },
        )
        .await
        .expect("send message");
    assert!(matches!(dispatched.outcome, SendMessageOutcome::Dispatched));
    settle(&client, created.session.session_id).await;

    let initial_queue = client
        .session_queue(created.session.session_id)
        .await
        .expect("initial queue");
    assert_eq!(
        initial_queue.inspection.runtime_state,
        SessionRuntimeState::WaitingOnApproval
    );
    assert_eq!(initial_queue.inspection.pending_approvals.len(), 1);
    assert_eq!(
        initial_queue.inspection.pending_approvals[0].tool_name,
        "shell"
    );

    let built_in_approval = client
        .approve_tool(
            created.session.session_id,
            &ApproveToolRequest {
                call_id: ToolCallId::new("call-shell-approval"),
                tool_name: "shell".to_owned(),
                scope: ApprovalScope::Once,
                decision: ApprovalDecision::Approved {
                    decided_at: time::OffsetDateTime::now_utc(),
                    decided_by: "test".to_owned(),
                    scope: ApprovalScope::Once,
                    source: ApprovalDecisionSource::Human,
                },
            },
        )
        .await
        .expect("approve built-in tool");
    assert_eq!(built_in_approval.status(), StatusCode::ACCEPTED);
    settle(&client, created.session.session_id).await;

    let mcp_queue = client
        .session_queue(created.session.session_id)
        .await
        .expect("mcp queue");
    assert_eq!(
        mcp_queue.inspection.runtime_state,
        SessionRuntimeState::WaitingOnApproval
    );
    assert_eq!(mcp_queue.inspection.pending_approvals.len(), 1);
    assert_eq!(
        mcp_queue.inspection.pending_approvals[0].tool_name,
        "mcp_fixture_echo_remote"
    );

    let mcp_approval = client
        .approve_tool(
            created.session.session_id,
            &ApproveToolRequest {
                call_id: ToolCallId::new("call-mcp-approval"),
                tool_name: "mcp_fixture_echo_remote".to_owned(),
                scope: ApprovalScope::Once,
                decision: ApprovalDecision::Approved {
                    decided_at: time::OffsetDateTime::now_utc(),
                    decided_by: "test".to_owned(),
                    scope: ApprovalScope::Once,
                    source: ApprovalDecisionSource::Human,
                },
            },
        )
        .await
        .expect("approve mcp tool");
    assert_eq!(mcp_approval.status(), StatusCode::ACCEPTED);
    settle(&client, created.session.session_id).await;

    let inspection = client
        .inspect_session(created.session.session_id)
        .await
        .expect("session inspection");
    assert_eq!(
        inspection.inspection.runtime_state,
        SessionRuntimeState::Idle
    );
    assert_eq!(inspection.inspection.pending_approval_count, 0);
    assert_eq!(inspection.inspection.pending_input_count, 0);
    assert_eq!(inspection.inspection.tool_call_count, 2);
    assert!(inspection.inspection.raw_chunk_count > 0);

    let final_queue = client
        .session_queue(created.session.session_id)
        .await
        .expect("final queue");
    assert_eq!(
        final_queue.inspection.runtime_state,
        SessionRuntimeState::Idle
    );
    assert!(final_queue.inspection.pending_approvals.is_empty());
    assert!(final_queue.inspection.pending_inputs.is_empty());
    assert!(!final_queue.inspection.cancel_requested);

    let execution = client
        .session_execution(created.session.session_id)
        .await
        .expect("execution inspection");
    assert_eq!(
        execution
            .inspection
            .turns
            .iter()
            .filter(|turn| matches!(turn.source, Some(TurnStartSource::ApprovalResume)))
            .count(),
        2
    );
    let execution_tools = execution
        .inspection
        .turns
        .iter()
        .flat_map(|turn| turn.tool_calls.iter())
        .map(|tool| (tool.call_id.to_string(), tool.tool_name.clone()))
        .collect::<Vec<_>>();
    assert!(
        execution_tools.iter().any(|(call_id, tool_name)| {
            call_id == "call-shell-approval" && tool_name == "shell"
        })
    );
    assert!(execution_tools.iter().any(|(call_id, tool_name)| {
        call_id == "call-mcp-approval" && tool_name == "mcp_fixture_echo_remote"
    }));

    let built_in_tool: SessionToolCallResponse = client
        .session_tool_call(
            created.session.session_id,
            &ToolCallId::new("call-shell-approval"),
        )
        .await
        .expect("built-in tool inspection");
    assert_eq!(built_in_tool.inspection.tool_name, "shell");
    assert_eq!(
        built_in_tool.inspection.approval_status.as_deref(),
        Some("approved")
    );
    assert_eq!(
        built_in_tool.inspection.execution_status.as_deref(),
        Some("completed")
    );

    let mcp_tool: SessionToolCallResponse = client
        .session_tool_call(
            created.session.session_id,
            &ToolCallId::new("call-mcp-approval"),
        )
        .await
        .expect("mcp tool inspection");
    assert_eq!(mcp_tool.inspection.tool_name, "mcp_fixture_echo_remote");
    assert_eq!(
        mcp_tool.inspection.approval_status.as_deref(),
        Some("approved")
    );
    assert_eq!(
        mcp_tool.inspection.execution_status.as_deref(),
        Some("completed")
    );

    let messages = client
        .session_messages(created.session.session_id)
        .await
        .expect("messages");
    assert!(messages.messages.iter().any(|message| matches!(
        message.parts.as_slice(),
        [MessagePart::ToolResult { result }]
            if result.call_id == ToolCallId::new("call-shell-approval")
                && result.output["stdout"] == "approved"
    )));
    assert!(messages.messages.iter().any(|message| matches!(
        message.parts.as_slice(),
        [MessagePart::ToolResult { result }]
            if result.call_id == ToolCallId::new("call-mcp-approval")
                && result.output["echoed"] == "remote hello"
    )));
    assert!(messages.messages.iter().any(|message| matches!(
        message.parts.as_slice(),
        [MessagePart::Text { text }]
            if message.role == Role::Assistant
                && text == "inspection contract complete"
    )));

    let commands = client
        .branch_operator_commands_page(
            created.session.session_id,
            created.branch.branch_id,
            None,
            10,
        )
        .await
        .expect("branch operator commands");
    assert!(commands.commands.iter().any(|command| {
        command.command_type == "slash_command"
            && command.raw_input == "/status"
            && command.output == "status output"
            && command.success
    }));

    let raw_chunks = client
        .raw_chunks(created.session.session_id)
        .await
        .expect("raw chunks");
    assert!(!raw_chunks.chunks.is_empty());

    let inventory = client.mcp_inventory().await.expect("mcp inventory");
    assert!(inventory.tools.iter().any(|tool| {
        tool.qualified_name == "mcp_fixture_echo_remote" && tool.server_name == "fixture"
    }));
    assert!(inventory.servers.iter().any(|server| {
        server.name == "fixture" && matches!(server.status, McpServerStatus::Ready)
    }));
    assert!(inventory.servers.iter().any(|server| {
        server.name == "broken" && matches!(server.status, McpServerStatus::Degraded { .. })
    }));

    let events = client
        .session_events(created.session.session_id, None)
        .await
        .expect("events");

    for call_id in ["call-shell-approval", "call-mcp-approval"] {
        let requested = events
            .events
            .iter()
            .filter(|event| {
                matches!(
                    &event.payload,
                    EventPayload::ToolCallRequested { call_id: recorded, .. }
                        if *recorded == ToolCallId::new(call_id)
                )
            })
            .collect::<Vec<_>>();
        let finished = events
            .events
            .iter()
            .filter(|event| {
                matches!(
                    &event.payload,
                    EventPayload::ToolExecutionFinished { call_id: recorded, .. }
                        if *recorded == ToolCallId::new(call_id)
                )
            })
            .collect::<Vec<_>>();
        let request_messages = events
            .events
            .iter()
            .filter(|event| {
                matches!(
                    &event.payload,
                    EventPayload::MessageAppended { message }
                        if message.tool_call().is_some_and(|call| {
                            call.call_id == call_id
                        })
                )
            })
            .collect::<Vec<_>>();
        let result_messages = events
            .events
            .iter()
            .filter(|event| {
                matches!(
                    &event.payload,
                    EventPayload::MessageAppended { message }
                        if message.tool_result().is_some_and(|result| {
                            result.call_id == ToolCallId::new(call_id)
                        })
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(requested.len(), 1, "one canonical request for {call_id}");
        assert_eq!(
            request_messages.len(),
            1,
            "one canonical tool-call message for {call_id}"
        );
        assert_eq!(finished.len(), 1, "one canonical terminal for {call_id}");
        assert_eq!(
            result_messages.len(),
            1,
            "one canonical tool-result message for {call_id}"
        );
        let next_completion = events
            .events
            .iter()
            .find(|event| {
                event.seq_id > finished[0].seq_id
                    && matches!(event.payload, EventPayload::CompletionRequested { .. })
            })
            .expect("tool result must be followed by provider continuation");
        let request_message = match &request_messages[0].payload {
            EventPayload::MessageAppended { message } => message,
            _ => unreachable!("filtered request message"),
        };
        let result_message = match &result_messages[0].payload {
            EventPayload::MessageAppended { message } => message,
            _ => unreachable!("filtered result message"),
        };
        let next_manifest = events
            .events
            .iter()
            .find_map(|event| {
                if event.seq_id > result_messages[0].seq_id
                    && event.seq_id < next_completion.seq_id
                    && let EventPayload::TurnContextManifestRecorded { manifest } = &event.payload
                {
                    Some(manifest)
                } else {
                    None
                }
            })
            .expect("provider continuation must record its model-visible context");
        assert!(
            requested[0].seq_id < request_messages[0].seq_id
                && request_messages[0].seq_id < finished[0].seq_id
                && finished[0].seq_id < result_messages[0].seq_id
                && result_messages[0].seq_id < next_completion.seq_id,
            "atomic tool evidence must be durable before provider continuation for {call_id}"
        );
        assert!(
            next_manifest.messages.iter().any(|message| {
                message.message_id == request_message.message_id
                    && message.source_seq_id == request_messages[0].seq_id
            }),
            "provider continuation must reuse the exact canonical tool-call message for {call_id}"
        );
        assert!(
            next_manifest.messages.iter().any(|message| {
                message.message_id == result_message.message_id
                    && message.source_seq_id == result_messages[0].seq_id
            }),
            "provider continuation must reuse the exact canonical tool-result message for {call_id}"
        );
    }

    assert!(events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::OperatorCommandRecorded {
            command_type,
            raw_input,
            output,
            success,
        } if command_type == "slash_command"
            && raw_input == "/status"
            && output == "status output"
            && *success
    )));
    assert!(events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolExecutionFinished { call_id, tool_name, result }
            if *call_id == ToolCallId::new("call-shell-approval")
                && tool_name == "shell"
                && result.output["stdout"] == "approved"
    )));
    assert!(events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolExecutionFinished { call_id, tool_name, result }
            if *call_id == ToolCallId::new("call-mcp-approval")
                && tool_name == "mcp_fixture_echo_remote"
                && result.output["echoed"] == "remote hello"
    )));

    let bundle = client
        .export_session(created.session.session_id, "legacy-bundle")
        .await
        .expect("bundle export");
    let bundle_document: Value = serde_json::from_str(&bundle.content).expect("valid bundle json");
    assert!(
        bundle_document["raw_chunks"]
            .as_array()
            .is_some_and(|chunks| !chunks.is_empty())
    );
    let bundle_serialized = serde_json::to_string(&bundle_document).expect("bundle string");
    assert!(bundle_serialized.contains("/status"));
    assert!(bundle_serialized.contains("call-shell-approval"));
    assert!(bundle_serialized.contains("call-mcp-approval"));

    let requests = mock_provider
        .requests
        .as_ref()
        .expect("captured requests")
        .lock()
        .expect("requests lock");
    assert_eq!(requests.len(), 3);
}

#[tokio::test]
async fn control_persistence_survives_restart_via_protocol_path() {
    let temp_dir = TempDir::new().expect("tempdir");
    let database_path = temp_dir.path().join("belltower.sqlite");
    let provider = spawn_mock_interrupt_provider(5_000, &["still running"]).await;
    let config = config_for_mock_provider(&provider.base_url);
    let first_server = spawn_server_with_database_path(config.clone(), &database_path, None).await;
    let client = api_client(&first_server);

    let created = client
        .create_session(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("control-persistence".to_owned()),
            objective: None,
            budget: None,
        })
        .await
        .expect("create session");

    let send_client = client.clone();
    let session_id = created.session.session_id;
    let branch_id = created.branch.branch_id;
    let send_handle = tokio::spawn(async move {
        send_client
            .send_message(
                session_id,
                &SendMessageRequest {
                    branch_id,
                    message: Message::text(Role::User, "start a long turn"),
                },
            )
            .await
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let steer = client
        .steer_session(
            created.session.session_id,
            &SteerSessionRequest {
                message: "keep going".to_owned(),
            },
        )
        .await
        .expect("steer request");
    assert_eq!(steer.status(), StatusCode::ACCEPTED);

    let cancel = client
        .cancel_session(
            created.session.session_id,
            &CancelSessionRequest {
                reason: Some("user request".to_owned()),
            },
        )
        .await
        .expect("cancel request");
    assert_eq!(cancel.status(), StatusCode::ACCEPTED);

    let mut queue = None;
    for _ in 0..20 {
        let candidate = client
            .session_queue(created.session.session_id)
            .await
            .expect("queue inspection");
        if candidate.inspection.cancel_requested && candidate.inspection.pending_steer_count == 1 {
            queue = Some(candidate);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let queue = queue.expect("pending controls should be visible before restart");
    assert_eq!(
        queue.inspection.runtime_state,
        SessionRuntimeState::CancelRequested
    );

    send_handle.abort();
    drop(first_server);

    let second_server = spawn_server_with_database_path(config, &database_path, None).await;
    let restarted_client = api_client(&second_server);

    let restarted_queue = restarted_client
        .session_queue(created.session.session_id)
        .await
        .expect("restarted queue inspection");
    assert!(restarted_queue.inspection.cancel_requested);
    assert_eq!(restarted_queue.inspection.pending_steer_count, 1);

    let restarted_events = restarted_client
        .session_events(created.session.session_id, None)
        .await
        .expect("restarted events");
    assert!(restarted_events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::SessionCancelled { reason } if reason == "user request"
    )));
    assert!(restarted_events.events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::SessionSteered { message, .. } if message == "keep going"
    )));
}

#[tokio::test]
async fn built_system_prompt_includes_runtime_context_instructions_and_plan() {
    let temp_dir = TempDir::new().expect("tempdir");
    let project_root =
        camino::Utf8PathBuf::from_path_buf(temp_dir.path().join("project")).expect("utf8");
    std::fs::create_dir_all(project_root.join(".belltower/skills")).expect("skills dir");
    std::fs::write(
        project_root.join("README.md").as_std_path(),
        "Project instructions.",
    )
    .expect("readme");
    std::fs::create_dir_all(project_root.join("tests")).expect("tests dir");
    std::fs::write(
        project_root.join("tests/test_outputs.py").as_std_path(),
        "def test_outputs(): pass",
    )
    .expect("test output file");
    std::fs::write(
        project_root
            .join(".belltower/skills/10-project.md")
            .as_std_path(),
        "Prefer precise summaries.",
    )
    .expect("project skill");

    let config = config_for_mock_provider("http://127.0.0.1:11434/v1");
    let database_path = temp_dir.path().join("sessions.sqlite");
    let runtime =
        Arc::new(BelltowerRuntime::open(config, &database_path).expect("runtime should open"));
    let state = AppState {
        runtime: runtime.clone(),
        token: Arc::new("test-token".to_owned()),
    };

    let (session, branch) = runtime
        .create_session(
            project_root.clone(),
            ConnectionId::new("local"),
            Some("qwen3:latest".to_owned()),
            SessionToolMode::Extended,
            None,
            Some("Summarize the workspace honestly.".to_owned()),
        )
        .expect("session");
    runtime
        .record_plan_updated(
            session.session_id,
            branch.branch_id,
            vec![bt_core::PlanItem {
                id: "p1".to_owned(),
                content: "Read Cargo.toml".to_owned(),
                status: bt_core::PlanStatus::InProgress,
            }],
            None,
        )
        .expect("plan");

    let tools = build_tool_registry(&state, &session, branch.branch_id)
        .await
        .expect("tools");
    let tool_specs = tools.specs();
    let instructions = runtime
        .resolve_instructions(Some(&session.project_root))
        .expect("instructions");
    let plan = runtime
        .current_plan(session.session_id, branch.branch_id)
        .expect("plan inspection");

    let built_prompt = build_session_system_prompt_with_provenance(
        &state,
        &session,
        &tool_specs,
        &instructions,
        plan.as_ref(),
    )
    .expect("prompt");
    let prompt = &built_prompt.prompt;

    assert!(prompt.contains("You are operating through Belltower"));
    assert!(
        built_prompt
            .core_prompt
            .source
            .ends_with("data/prompts/00-core.md")
    );
    assert!(built_prompt.provider_overlay.is_some());
    assert!(
        prompt.contains(
            "After receiving tool results, continue the turn and answer the user directly"
        )
    );
    assert!(prompt.contains(
        "If a tool returns an error, inspect the error, adjust the call if recovery is obvious"
    ));
    assert!(prompt.contains(
        "For file-producing, data-processing, or exact-output tasks, inspect local instructions"
    ));
    assert!(prompt.contains("inspect the relevant surfaces before using write/edit"));
    assert!(
        prompt.contains(
            "For tabular or structured data tasks, align records by explicit identifiers"
        )
    );
    assert!(prompt.contains("Objective:\nSummarize the workspace honestly."));
    assert!(prompt.contains("- cwd: "));
    assert!(prompt.contains(&project_root.to_string()));
    assert!(prompt.contains("- connection: local"));
    assert!(prompt.contains("- model: qwen3:latest"));
    assert!(prompt.contains("- tool mode: extended"));
    assert!(prompt.contains("- validation surfaces detected:"));
    assert!(prompt.contains("  - README.md"));
    assert!(prompt.contains("  - tests/"));
    assert!(prompt.contains("  - tests/test_outputs.py"));
    assert!(prompt.contains("- `web_search`: discover external web sources"));
    assert!(prompt.contains("- `web_fetch`: retrieve public URL content"));
    assert!(prompt.contains("- `plan`: track multi-step work explicitly"));
    assert!(prompt.contains("- `ask`: pause and ask the human"));
    assert!(prompt.contains("- `inspect`: check prior harness state"));
    assert!(prompt.contains("Provider notes (openai-compatible):"));
    assert!(prompt.contains("## 10-project"));
    assert!(prompt.contains("Prefer precise summaries."));
    assert!(prompt.contains("Active plan:"));
    assert!(prompt.contains("Read Cargo.toml"));
}

#[tokio::test]
async fn approval_round_trip_executes_tool_and_continues_turn() {
    let mock_provider = spawn_mock_approval_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();
    let project_root = tempfile::TempDir::new().expect("project root");

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: project_root.path().display().to_string(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("approval".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create response");

    let send = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "run a command"),
        })
        .send()
        .await
        .expect("send message");
    assert_eq!(send.status(), StatusCode::ACCEPTED);
    let api = api_client(&server);
    assert_eq!(
        settle(&api, created.session.session_id).await,
        SessionRuntimeState::WaitingOnApproval
    );

    let paused_messages: SessionMessagesResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/messages", created.session.session_id),
        )
        .send()
        .await
        .expect("messages request")
        .json()
        .await
        .expect("messages body");
    assert!(paused_messages.messages.iter().any(|message| matches!(
        message.tool_call(),
        Some(call) if call.tool_name == "shell"
    )));
    assert!(
        !paused_messages
            .messages
            .iter()
            .any(|message| message.tool_result().is_some())
    );

    let approval = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/approve", created.session.session_id),
        )
        .json(&ApproveToolRequest {
            call_id: bt_core::ToolCallId::new("call-approval"),
            tool_name: "shell".to_owned(),
            scope: bt_core::ApprovalScope::Once,
            decision: bt_core::ApprovalDecision::Approved {
                decided_at: time::OffsetDateTime::now_utc(),
                decided_by: "test".to_owned(),
                scope: bt_core::ApprovalScope::Once,
                source: bt_core::ApprovalDecisionSource::Human,
            },
        })
        .send()
        .await
        .expect("approve tool");
    assert_eq!(approval.status(), StatusCode::ACCEPTED);
    settle(&api, created.session.session_id).await;

    let resumed_messages: SessionMessagesResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/messages", created.session.session_id),
        )
        .send()
        .await
        .expect("messages request")
        .json()
        .await
        .expect("messages body");
    assert!(resumed_messages.messages.iter().any(|message| matches!(
        message.tool_result(),
        Some(result)
            if result.tool_name == "shell"
                && result.call_id.to_string() == "call-approval"
                && result.output["stdout"] == "approved"
    )));
    assert!(resumed_messages.messages.iter().any(|message| matches!(
        message.parts.as_slice(),
        [MessagePart::Text { text }]
            if message.role == Role::Assistant && text == "complete"
    )));

    {
        let requests = mock_provider
            .requests
            .as_ref()
            .expect("captured requests")
            .lock()
            .expect("requests lock");
        assert_eq!(requests.len(), 2);

        let second_request = &requests[1];
        let messages = second_request["messages"]
            .as_array()
            .expect("messages should be an array");
        assert!(messages.iter().any(|message| {
            message["role"] == "assistant"
                && (message["content"].is_null() || message["content"] == "")
                && message["tool_calls"].is_array()
                && message["tool_calls"][0]["id"] == "call-approval"
                && message["tool_calls"][0]["type"] == "function"
                && message["tool_calls"][0]["function"]["name"] == "shell"
                && !message["tool_calls"][0]["function"]["arguments"]
                    .as_str()
                    .expect("function arguments string")
                    .contains("model-call-id")
        }));
        assert!(messages.iter().any(|message| {
            message["role"] == "tool"
                && message["tool_call_id"] == "call-approval"
                && message["content"].is_string()
        }));
    }

    let events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("events request")
        .json()
        .await
        .expect("events body");
    let resumed_start = events
        .events
        .iter()
        .find(|event| {
            matches!(
                &event.payload,
                EventPayload::TurnStarted {
                    source: bt_core::TurnStartSource::ApprovalResume,
                    resumed_from_call_id: Some(call_id),
                    ..
                } if *call_id == ToolCallId::new("call-approval")
            )
        })
        .expect("approval resume turn.started");
    let resumed_turn_id = resumed_start.turn_id.expect("resumed turn id");
    let resumed_start_seq = resumed_start.seq_id.expect("resumed turn seq");
    let approval_seq = events
        .events
        .iter()
        .find(|event| {
            matches!(
                &event.payload,
                EventPayload::ToolApprovalResolved { call_id, .. }
                    if *call_id == ToolCallId::new("call-approval")
                        && event.turn_id == Some(resumed_turn_id)
            )
        })
        .and_then(|event| event.seq_id)
        .expect("approval resolved seq");
    let tool_result_seq = events
        .events
        .iter()
        .find(|event| {
            matches!(
                &event.payload,
                EventPayload::ToolExecutionFinished { call_id, .. }
                    if *call_id == ToolCallId::new("call-approval")
                        && event.turn_id == Some(resumed_turn_id)
            )
        })
        .and_then(|event| event.seq_id)
        .expect("tool execution seq");
    assert!(resumed_start_seq < approval_seq);
    assert!(approval_seq < tool_result_seq);
    assert_eq!(
        events
            .events
            .iter()
            .filter(|event| {
                matches!(
                    &event.payload,
                    EventPayload::TurnStarted { turn_id, .. }
                        if *turn_id == resumed_turn_id
                )
            })
            .count(),
        1
    );

    let otlp: SessionExportResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/export/otlp", created.session.session_id),
        )
        .send()
        .await
        .expect("otlp export request")
        .json()
        .await
        .expect("otlp export body");
    let document: Value = serde_json::from_str(&otlp.content).expect("valid otlp export");
    let spans = document["resourceSpans"][0]["scopeSpans"][0]["spans"]
        .as_array()
        .expect("spans array");
    let turn_spans = spans.iter().filter(|span| span["name"] == "turn").count();
    assert_eq!(
        turn_spans, 2,
        "expected only the original and resumed turns"
    );
    let resumed_tool_spans = spans
        .iter()
        .filter(|span| {
            span["name"] == "tool_execution"
                && span["attributes"].as_array().is_some_and(|attributes| {
                    attributes.iter().any(|attribute| {
                        attribute["key"] == "turn.id"
                            && attribute["value"]["stringValue"] == resumed_turn_id.to_string()
                    })
                })
        })
        .count();
    assert_eq!(resumed_tool_spans, 1, "expected one resumed tool span");
}

#[tokio::test]
async fn once_approval_does_not_authorize_repeated_identical_request() {
    let mock_provider = spawn_mock_repeated_approval_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();
    let project_root = tempfile::TempDir::new().expect("project root");

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: project_root.path().display().to_string(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("approval-once-repeated-request".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let send = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "run the command"),
        })
        .send()
        .await
        .expect("send message");
    assert_eq!(send.status(), StatusCode::ACCEPTED);
    let api = api_client(&server);
    assert_eq!(
        settle(&api, created.session.session_id).await,
        SessionRuntimeState::WaitingOnApproval
    );

    let call_id = ToolCallId::new("call-approval-repeat");
    let approval = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/approve", created.session.session_id),
        )
        .json(&ApproveToolRequest {
            call_id: call_id.clone(),
            tool_name: "shell".to_owned(),
            scope: ApprovalScope::Once,
            decision: ApprovalDecision::Approved {
                decided_at: time::OffsetDateTime::now_utc(),
                decided_by: "test".to_owned(),
                scope: ApprovalScope::Once,
                source: ApprovalDecisionSource::Human,
            },
        })
        .send()
        .await
        .expect("approve tool");
    assert_eq!(approval.status(), StatusCode::ACCEPTED);
    assert_eq!(
        settle(&api, created.session.session_id).await,
        SessionRuntimeState::WaitingOnApproval
    );

    let events = server
        .runtime
        .all_events(created.session.session_id)
        .expect("canonical events");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                &event.payload,
                EventPayload::ToolApprovalRequested { call_id: recorded, .. }
                    if *recorded == call_id
            ))
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                &event.payload,
                EventPayload::ToolApprovalResolved { call_id: recorded, .. }
                    if *recorded == call_id
            ))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                &event.payload,
                EventPayload::ToolExecutionFinished { call_id: recorded, .. }
                    if *recorded == call_id
            ))
            .count(),
        1
    );
    assert!(
        server
            .runtime
            .resumable_approval_call(created.session.session_id, call_id, "shell")
            .expect("resumable approval lookup")
            .is_some()
    );
    assert_eq!(
        mock_provider
            .requests
            .as_ref()
            .expect("captured requests")
            .lock()
            .expect("requests lock")
            .len(),
        2
    );
}

#[tokio::test]
async fn approval_resume_suspends_sibling_tool_calls_from_same_response() {
    let mock_provider = spawn_mock_dual_approval_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();
    let project_root = tempfile::TempDir::new().expect("project root");

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: project_root.path().display().to_string(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("dual-approval".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create response");

    let send = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "run two commands"),
        })
        .send()
        .await
        .expect("send message");
    assert_eq!(send.status(), StatusCode::ACCEPTED);
    let api = api_client(&server);
    assert_eq!(
        settle(&api, created.session.session_id).await,
        SessionRuntimeState::WaitingOnApproval
    );

    let initial_queue: SessionQueueResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/queue", created.session.session_id),
        )
        .send()
        .await
        .expect("queue request")
        .json()
        .await
        .expect("queue body");
    assert_eq!(initial_queue.inspection.pending_approvals.len(), 1);
    assert_eq!(
        initial_queue.inspection.pending_approvals[0].call_id,
        bt_core::ToolCallId::new("call-approval-one")
    );

    let first_approval = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/approve", created.session.session_id),
        )
        .json(&ApproveToolRequest {
            call_id: bt_core::ToolCallId::new("call-approval-one"),
            tool_name: "shell".to_owned(),
            scope: bt_core::ApprovalScope::Once,
            decision: bt_core::ApprovalDecision::Approved {
                decided_at: time::OffsetDateTime::now_utc(),
                decided_by: "test".to_owned(),
                scope: bt_core::ApprovalScope::Once,
                source: bt_core::ApprovalDecisionSource::Human,
            },
        })
        .send()
        .await
        .expect("approve first tool");
    assert_eq!(first_approval.status(), StatusCode::ACCEPTED);
    settle(&api, created.session.session_id).await;

    {
        let requests = mock_provider
            .requests
            .as_ref()
            .expect("captured requests")
            .lock()
            .expect("requests lock");
        assert_eq!(requests.len(), 2);
        let resumed_messages = requests[1]["messages"]
            .as_array()
            .expect("resumed request messages");
        assert!(
            resumed_messages.iter().any(|message| {
                message["role"] == "tool" && message["tool_call_id"] == "call-approval-one"
            }),
            "resumed provider request is missing first tool output"
        );
        assert!(
            !resumed_messages.iter().any(|message| {
                message["role"] == "tool" && message["tool_call_id"] == "call-approval-two"
            }),
            "sibling tool call should not be recorded before approval resumes the turn"
        );
    }

    let final_queue: SessionQueueResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/queue", created.session.session_id),
        )
        .send()
        .await
        .expect("queue request")
        .json()
        .await
        .expect("queue body");
    assert!(final_queue.inspection.pending_approvals.is_empty());

    let final_messages: SessionMessagesResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/messages", created.session.session_id),
        )
        .send()
        .await
        .expect("messages request")
        .json()
        .await
        .expect("messages body");
    assert!(final_messages.messages.iter().any(|message| matches!(
        message.parts.as_slice(),
        [MessagePart::Text { text }]
            if message.role == Role::Assistant && text == "complete"
    )));
}

#[tokio::test]
async fn approval_round_trip_resumes_pending_call_on_original_branch() {
    let mock_provider = spawn_mock_approval_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();
    let project_root = tempfile::TempDir::new().expect("project root");

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: project_root.path().display().to_string(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("approval-branch".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create response");

    let child: CreateBranchResponse = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/branches", created.session.session_id),
        )
        .json(&CreateBranchRequest {
            from_branch_id: created.branch.branch_id,
            from_event_id: None,
            activate: false,
            carry_summary: false,
        })
        .send()
        .await
        .expect("create branch")
        .json()
        .await
        .expect("create branch body");

    let send = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: child.branch.branch_id,
            message: Message::text(Role::User, "run a command"),
        })
        .send()
        .await
        .expect("send message");
    assert_eq!(send.status(), StatusCode::ACCEPTED);
    let api = api_client(&server);
    assert_eq!(
        settle(&api, created.session.session_id).await,
        SessionRuntimeState::WaitingOnApproval
    );

    let paused_messages: SessionMessagesResponse = server
        .request(
            &client,
            Method::GET,
            &format!(
                "sessions/{}/branches/{}/messages",
                created.session.session_id, child.branch.branch_id
            ),
        )
        .send()
        .await
        .expect("branch messages request")
        .json()
        .await
        .expect("branch messages body");
    assert!(paused_messages.messages.iter().any(|message| matches!(
        message.tool_call(),
        Some(call) if call.tool_name == "shell"
    )));
    assert!(
        !paused_messages
            .messages
            .iter()
            .any(|message| message.tool_result().is_some())
    );

    let approval = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/approve", created.session.session_id),
        )
        .json(&ApproveToolRequest {
            call_id: bt_core::ToolCallId::new("call-approval"),
            tool_name: "shell".to_owned(),
            scope: bt_core::ApprovalScope::Once,
            decision: bt_core::ApprovalDecision::Approved {
                decided_at: time::OffsetDateTime::now_utc(),
                decided_by: "test".to_owned(),
                scope: bt_core::ApprovalScope::Once,
                source: bt_core::ApprovalDecisionSource::Human,
            },
        })
        .send()
        .await
        .expect("approve tool");
    assert_eq!(approval.status(), StatusCode::ACCEPTED);
    settle(&api, created.session.session_id).await;

    let resumed_messages: SessionMessagesResponse = server
        .request(
            &client,
            Method::GET,
            &format!(
                "sessions/{}/branches/{}/messages",
                created.session.session_id, child.branch.branch_id
            ),
        )
        .send()
        .await
        .expect("branch messages request")
        .json()
        .await
        .expect("branch messages body");
    assert!(resumed_messages.messages.iter().any(|message| matches!(
        message.tool_result(),
        Some(result)
            if result.tool_name == "shell"
                && result.call_id.to_string() == "call-approval"
                && result.output["stdout"] == "approved"
    )));
    assert!(resumed_messages.messages.iter().any(|message| matches!(
        message.parts.as_slice(),
        [MessagePart::Text { text }]
            if message.role == Role::Assistant && text == "complete"
    )));

    let events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("events request")
        .json()
        .await
        .expect("events body");
    let resumed_start = events
        .events
        .iter()
        .find(|event| {
            matches!(
                &event.payload,
                EventPayload::TurnStarted {
                    source: bt_core::TurnStartSource::ApprovalResume,
                    resumed_from_call_id: Some(call_id),
                    ..
                } if *call_id == ToolCallId::new("call-approval")
            )
        })
        .expect("approval resume turn.started");
    assert_eq!(resumed_start.branch_id, child.branch.branch_id);
}

#[tokio::test]
async fn approval_resume_keeps_paused_settings_revision_after_session_model_change() {
    let mock_provider = spawn_mock_approval_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();
    let project_root = tempfile::TempDir::new().expect("project root");

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: project_root.path().display().to_string(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("approval-settings".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create response");

    let paused_settings_revision_id = created.session.settings_revision_id;

    let send = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "run a command"),
        })
        .send()
        .await
        .expect("send message");
    assert_eq!(send.status(), StatusCode::ACCEPTED);
    let api = api_client(&server);
    assert_eq!(
        settle(&api, created.session.session_id).await,
        SessionRuntimeState::WaitingOnApproval
    );

    let updated: CreateSessionResponse = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}", created.session.session_id),
        )
        .json(&UpdateSessionRequest {
            connection_id: None,
            model_id: Some("updated-model".to_owned()),
            tool_mode: None,
            reset_model_to_default: false,
        })
        .send()
        .await
        .expect("update session")
        .json()
        .await
        .expect("update session body");
    assert_eq!(
        updated.session.settings_revision_id,
        paused_settings_revision_id + 1
    );

    let approval = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/approve", created.session.session_id),
        )
        .json(&ApproveToolRequest {
            call_id: bt_core::ToolCallId::new("call-approval"),
            tool_name: "shell".to_owned(),
            scope: bt_core::ApprovalScope::Once,
            decision: bt_core::ApprovalDecision::Approved {
                decided_at: time::OffsetDateTime::now_utc(),
                decided_by: "test".to_owned(),
                scope: bt_core::ApprovalScope::Once,
                source: bt_core::ApprovalDecisionSource::Human,
            },
        })
        .send()
        .await
        .expect("approve tool");
    assert_eq!(approval.status(), StatusCode::ACCEPTED);
    settle(&api, created.session.session_id).await;

    let events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("events request")
        .json()
        .await
        .expect("events body");
    let resumed_start = events
        .events
        .iter()
        .find(|event| {
            matches!(
                &event.payload,
                EventPayload::TurnStarted {
                    source: bt_core::TurnStartSource::ApprovalResume,
                    resumed_from_call_id: Some(call_id),
                    ..
                } if *call_id == ToolCallId::new("call-approval")
            )
        })
        .expect("approval resume turn.started");
    assert!(matches!(
        &resumed_start.payload,
        EventPayload::TurnStarted {
            settings_revision_id,
            ..
        } if *settings_revision_id == paused_settings_revision_id
    ));
}

#[tokio::test]
async fn failed_approved_tool_resume_records_one_terminal_transition() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();
    let (session, branch) = server
        .runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("failed-approval-resume".to_owned()),
            None,
        )
        .expect("create session");
    let paused_turn_id = TurnId::new();
    let call_id = ToolCallId::new("call-unavailable-tool");
    let tool_name = "unavailable_tool".to_owned();
    let arguments = serde_json::json!({"value": "test"});
    let tool_call = ToolCall {
        tool_name: tool_name.clone(),
        call_id: call_id.to_string(),
        arguments: arguments.clone(),
    };
    let approval_request = bt_core::ApprovalRequest {
        session_id: session.session_id,
        call_id: call_id.clone(),
        tool_name: tool_name.clone(),
        arguments: arguments.clone(),
        requirement: bt_core::ApprovalRequirement::Always,
        tool_metadata: bt_core::ToolMetadata {
            risk_class: bt_core::ToolRiskClass::Moderate,
            is_read_only: false,
            is_concurrency_safe: false,
            interrupt_behavior: bt_core::ToolInterruptBehavior::Immediate,
            execution_mode: bt_core::ToolExecutionMode::Immediate,
            should_defer: false,
            catalogue_tags: vec!["test".to_owned()],
            display_group: bt_core::ToolDisplayGroup::External,
        },
        requested_at: time::OffsetDateTime::now_utc(),
    };

    server
        .runtime
        .configure_session_budget(
            session.session_id,
            branch.branch_id,
            BudgetConfig {
                max_turns: Some(1),
                ..BudgetConfig::default()
            },
        )
        .expect("configure resumed-turn budget");

    server
        .runtime
        .append_message(&session, &branch, Role::User, "run the unavailable tool")
        .expect("append user message");
    server
        .runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "test-model".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("record paused turn");
    server
        .runtime
        .append_raw_message(
            &session,
            &branch,
            Message::from_part(
                Role::Assistant,
                MessagePart::ToolCall {
                    call: tool_call.clone(),
                },
            ),
            Some(paused_turn_id),
        )
        .expect("append tool call message");
    server
        .runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            tool_name.clone(),
            arguments,
            Some(paused_turn_id),
        )
        .expect("record tool request");
    server
        .runtime
        .record_approval_requested_for_request(
            session.session_id,
            branch.branch_id,
            &approval_request,
            Some(paused_turn_id),
        )
        .expect("record approval request");
    server
        .runtime
        .record_turn_finished(
            session.session_id,
            branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "test-model".to_owned(),
            "awaiting_approval".to_owned(),
            Some("ToolCalls".to_owned()),
            1,
        )
        .expect("finish paused turn");

    let response = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/approve", session.session_id),
        )
        .json(&ApproveToolRequest {
            call_id: call_id.clone(),
            tool_name: tool_name.clone(),
            scope: ApprovalScope::Once,
            decision: ApprovalDecision::Approved {
                decided_at: time::OffsetDateTime::now_utc(),
                decided_by: "test".to_owned(),
                scope: ApprovalScope::Once,
                source: ApprovalDecisionSource::Human,
            },
        })
        .send()
        .await
        .expect("approve tool");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    settle(&api_client(&server), session.session_id).await;

    let events = server
        .runtime
        .all_events(session.session_id)
        .expect("canonical events");
    let terminal_index = events
        .iter()
        .position(|event| {
            matches!(
                &event.payload,
                EventPayload::ToolExecutionFinished { call_id: recorded, result, .. }
                    if *recorded == call_id && result.is_error
            )
        })
        .expect("terminal tool event");
    let result_message_index = events
        .iter()
        .position(|event| {
            matches!(
                &event.payload,
                EventPayload::MessageAppended { message }
                    if matches!(
                        message.tool_result(),
                        Some(result) if result.call_id == call_id && result.is_error
                    )
            )
        })
        .expect("error tool result message");
    let session_error_index = events
        .iter()
        .position(|event| {
            matches!(
                &event.payload,
                EventPayload::SessionError {
                    class: ErrorClass::Tool,
                    ..
                }
            )
        })
        .expect("session error");
    let turn_finished_index = events
        .iter()
        .position(|event| {
            matches!(
                &event.payload,
                EventPayload::TurnFinished { status, .. } if status == "failed"
            )
        })
        .expect("failed turn finish");
    let checkpoint_index = events
        .iter()
        .position(|event| {
            matches!(event.payload, EventPayload::BudgetCheckpoint { .. })
                && event.turn_id == events[turn_finished_index].turn_id
        })
        .expect("resumed failure budget checkpoint");
    let cancellation_index = events
        .iter()
        .position(|event| {
            matches!(
                &event.payload,
                EventPayload::SessionCancelled { reason } if reason == "budget_exhausted"
            )
        })
        .expect("resumed failure budget cancellation");
    assert!(checkpoint_index < cancellation_index);
    assert!(cancellation_index < terminal_index);
    assert!(terminal_index < result_message_index);
    assert!(result_message_index < session_error_index);
    assert!(session_error_index < turn_finished_index);
    let transition_turn_id = events[terminal_index].turn_id;
    assert!(
        [
            terminal_index,
            result_message_index,
            session_error_index,
            turn_finished_index,
        ]
        .into_iter()
        .all(|index| events[index].turn_id == transition_turn_id)
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                matches!(
                    &event.payload,
                    EventPayload::ToolExecutionFinished { call_id: recorded, .. }
                        if *recorded == call_id
                )
            })
            .count(),
        1
    );

    let queue = server
        .runtime
        .inspect_queue(session.session_id)
        .expect("queue inspection")
        .expect("session queue");
    assert!(queue.pending_approvals.is_empty());
    let inspection = server
        .runtime
        .inspect_tool_call(session.session_id, call_id)
        .expect("tool inspection")
        .expect("recorded tool call");
    assert_eq!(inspection.approval_status.as_deref(), Some("approved"));
    assert_eq!(inspection.execution_status.as_deref(), Some("completed"));
    assert_eq!(
        inspection
            .result
            .as_ref()
            .and_then(|value| value.get("is_error")),
        Some(&Value::Bool(true))
    );
}

#[tokio::test]
async fn approved_tool_resume_closes_turn_when_initial_terminal_append_fails() {
    let server = spawn_server(BelltowerConfig::from_embedded().expect("config")).await;
    let client = Client::new();
    let project_root = tempfile::TempDir::new().expect("project root");
    let marker = project_root.path().join("tool-started");
    let command = format!("touch '{}' && sleep 1", marker.display());
    let (session, branch) = server
        .runtime
        .create_session(
            project_root.path().display().to_string().into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("terminal-persistence-recovery".to_owned()),
            None,
        )
        .expect("create session");
    let paused_turn_id = TurnId::new();
    let call_id = ToolCallId::new("call-shell-persistence");
    let arguments = serde_json::json!({
        "command": command,
        "timeout_seconds": 5,
    });
    let tool_call = ToolCall {
        tool_name: "shell".to_owned(),
        call_id: call_id.to_string(),
        arguments: arguments.clone(),
    };
    let approval_request = bt_core::ApprovalRequest {
        session_id: session.session_id,
        call_id: call_id.clone(),
        tool_name: "shell".to_owned(),
        arguments: arguments.clone(),
        requirement: bt_core::ApprovalRequirement::Always,
        tool_metadata: bt_core::ToolMetadata {
            risk_class: bt_core::ToolRiskClass::High,
            is_read_only: false,
            is_concurrency_safe: false,
            interrupt_behavior: bt_core::ToolInterruptBehavior::TerminateProcess,
            execution_mode: bt_core::ToolExecutionMode::Immediate,
            should_defer: false,
            catalogue_tags: vec!["execution".to_owned()],
            display_group: bt_core::ToolDisplayGroup::Execution,
        },
        requested_at: time::OffsetDateTime::now_utc(),
    };

    server
        .runtime
        .append_message(&session, &branch, Role::User, "run the shell command")
        .expect("append user message");
    server
        .runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "test-model".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("record paused turn");
    server
        .runtime
        .append_raw_message(
            &session,
            &branch,
            Message::from_part(Role::Assistant, MessagePart::ToolCall { call: tool_call }),
            Some(paused_turn_id),
        )
        .expect("append tool call message");
    server
        .runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "shell".to_owned(),
            arguments,
            Some(paused_turn_id),
        )
        .expect("record tool request");
    server
        .runtime
        .record_approval_requested_for_request(
            session.session_id,
            branch.branch_id,
            &approval_request,
            Some(paused_turn_id),
        )
        .expect("record approval request");
    server
        .runtime
        .record_turn_finished(
            session.session_id,
            branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "test-model".to_owned(),
            "awaiting_approval".to_owned(),
            Some("ToolCalls".to_owned()),
            1,
        )
        .expect("finish paused turn");

    let response = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/approve", session.session_id),
        )
        .json(&ApproveToolRequest {
            call_id: call_id.clone(),
            tool_name: "shell".to_owned(),
            scope: ApprovalScope::Once,
            decision: ApprovalDecision::Approved {
                decided_at: time::OffsetDateTime::now_utc(),
                decided_by: "test".to_owned(),
                scope: ApprovalScope::Once,
                source: ApprovalDecisionSource::Human,
            },
        })
        .send()
        .await
        .expect("approve tool");
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !marker.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("tool execution should start");
    server
        .runtime
        .inject_next_store_append_error_for_test("terminal append failure");

    settle(&api_client(&server), session.session_id).await;
    let events = server
        .runtime
        .all_events(session.session_id)
        .expect("canonical events");
    let resumed_turn_id = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::TurnStarted {
                source: TurnStartSource::ApprovalResume,
                resumed_from_call_id: Some(resumed_call_id),
                ..
            } if *resumed_call_id == call_id => event.turn_id,
            _ => None,
        })
        .expect("resumed turn");
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event.turn_id == Some(resumed_turn_id)
                    && matches!(
                        &event.payload,
                        EventPayload::ToolExecutionFinished { call_id: recorded, .. }
                            if *recorded == call_id
                    )
            })
            .count(),
        1
    );
    assert!(events.iter().any(|event| {
        event.turn_id == Some(resumed_turn_id)
            && matches!(
                &event.payload,
                EventPayload::SessionError {
                    class: ErrorClass::Storage,
                    ..
                }
            )
    }));
    assert!(events.iter().any(|event| {
        event.turn_id == Some(resumed_turn_id)
            && matches!(
                &event.payload,
                EventPayload::TurnFinished { status, .. } if status == "failed"
            )
    }));
}

#[tokio::test]
async fn answer_round_trip_starts_resumed_turn_before_result_events() {
    let mock_provider = spawn_mock_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("answer".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let paused_turn_id = TurnId::new();
    let ask_call = ToolCall {
        tool_name: "ask".to_owned(),
        call_id: "call-ask".to_owned(),
        arguments: serde_json::json!({
            "question": "Continue?",
            "choices": ["yes", "no"],
        }),
    };
    server
        .runtime
        .append_message(&created.session, &created.branch, Role::User, "need input")
        .expect("append user message");
    server
        .runtime
        .record_turn_started(
            created.session.session_id,
            created.branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            created.session.settings_revision_id,
            bt_core::TurnStartSource::UserMessage,
            None,
        )
        .expect("paused turn started");
    server
        .runtime
        .record_tool_call_requested(
            created.session.session_id,
            created.branch.branch_id,
            ToolCallId::new("call-ask"),
            "ask".to_owned(),
            ask_call.arguments.clone(),
            Some(paused_turn_id),
        )
        .expect("tool call requested");
    server
        .runtime
        .append_raw_message(
            &created.session,
            &created.branch,
            Message::from_part(Role::Assistant, MessagePart::ToolCall { call: ask_call }),
            Some(paused_turn_id),
        )
        .expect("append ask tool call");
    server
        .runtime
        .record_turn_finished(
            created.session.session_id,
            created.branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            "awaiting_input".to_owned(),
            Some("ToolCalls".to_owned()),
            1,
        )
        .expect("finish paused turn");

    let answer = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/answer", created.session.session_id),
        )
        .json(&AnswerToolRequest {
            call_id: ToolCallId::new("call-ask"),
            response: Value::String("yes".to_owned()),
        })
        .send()
        .await
        .expect("answer tool");
    assert_eq!(answer.status(), StatusCode::ACCEPTED);

    let events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("events request")
        .json()
        .await
        .expect("events body");
    let resumed_start = events
        .events
        .iter()
        .find(|event| {
            matches!(
                &event.payload,
                EventPayload::TurnStarted {
                    source: bt_core::TurnStartSource::InputResume,
                    resumed_from_call_id: Some(call_id),
                    ..
                } if *call_id == ToolCallId::new("call-ask")
            )
        })
        .expect("input resume turn.started");
    let resumed_turn_id = resumed_start.turn_id.expect("resumed turn id");
    let resumed_start_seq = resumed_start.seq_id.expect("resumed start seq");
    let tool_message_seq = events
        .events
        .iter()
        .find(|event| {
            matches!(
                &event.payload,
                EventPayload::MessageAppended { message }
                    if event.turn_id == Some(resumed_turn_id)
                        && message.tool_result().is_some_and(|result| {
                            result.call_id == ToolCallId::new("call-ask")
                        })
            )
        })
        .and_then(|event| event.seq_id)
        .expect("tool result message seq");
    let tool_execution_seq = events
        .events
        .iter()
        .find(|event| {
            matches!(
                &event.payload,
                EventPayload::ToolExecutionFinished { call_id, .. }
                    if *call_id == ToolCallId::new("call-ask")
                        && event.turn_id == Some(resumed_turn_id)
            )
        })
        .and_then(|event| event.seq_id)
        .expect("tool execution seq");
    assert!(resumed_start_seq < tool_execution_seq);
    assert!(tool_execution_seq < tool_message_seq);
    assert_eq!(
        events
            .events
            .iter()
            .filter(|event| {
                matches!(
                    &event.payload,
                    EventPayload::TurnStarted { turn_id, .. }
                        if *turn_id == resumed_turn_id
                )
            })
            .count(),
        1
    );
}

#[tokio::test]
async fn answer_round_trip_resumes_pending_call_on_original_branch() {
    let mock_provider = spawn_mock_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("answer-branch".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let child: CreateBranchResponse = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/branches", created.session.session_id),
        )
        .json(&CreateBranchRequest {
            from_branch_id: created.branch.branch_id,
            from_event_id: None,
            activate: false,
            carry_summary: false,
        })
        .send()
        .await
        .expect("create branch")
        .json()
        .await
        .expect("create branch body");

    let paused_turn_id = TurnId::new();
    let ask_call = ToolCall {
        tool_name: "ask".to_owned(),
        call_id: "call-ask".to_owned(),
        arguments: serde_json::json!({
            "question": "Continue?",
            "choices": ["yes", "no"],
        }),
    };
    server
        .runtime
        .append_message(&created.session, &child.branch, Role::User, "need input")
        .expect("append user message");
    server
        .runtime
        .record_turn_started(
            created.session.session_id,
            child.branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            created.session.settings_revision_id,
            bt_core::TurnStartSource::UserMessage,
            None,
        )
        .expect("paused turn started");
    server
        .runtime
        .record_tool_call_requested(
            created.session.session_id,
            child.branch.branch_id,
            ToolCallId::new("call-ask"),
            "ask".to_owned(),
            ask_call.arguments.clone(),
            Some(paused_turn_id),
        )
        .expect("tool call requested");
    server
        .runtime
        .append_raw_message(
            &created.session,
            &child.branch,
            Message::from_part(Role::Assistant, MessagePart::ToolCall { call: ask_call }),
            Some(paused_turn_id),
        )
        .expect("append ask tool call");
    server
        .runtime
        .record_turn_finished(
            created.session.session_id,
            child.branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            "awaiting_input".to_owned(),
            Some("ToolCalls".to_owned()),
            1,
        )
        .expect("finish paused turn");

    let answer = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/answer", created.session.session_id),
        )
        .json(&AnswerToolRequest {
            call_id: ToolCallId::new("call-ask"),
            response: Value::String("yes".to_owned()),
        })
        .send()
        .await
        .expect("answer tool");
    assert_eq!(answer.status(), StatusCode::ACCEPTED);

    let branch_messages: SessionMessagesResponse = server
        .request(
            &client,
            Method::GET,
            &format!(
                "sessions/{}/branches/{}/messages",
                created.session.session_id, child.branch.branch_id
            ),
        )
        .send()
        .await
        .expect("branch messages request")
        .json()
        .await
        .expect("branch messages body");
    assert!(branch_messages.messages.iter().any(|message| matches!(
        message.tool_result(),
        Some(result)
            if result.call_id == ToolCallId::new("call-ask")
                && result.output["response"] == Value::String("yes".to_owned())
    )));

    let events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("events request")
        .json()
        .await
        .expect("events body");
    let resumed_start = events
        .events
        .iter()
        .find(|event| {
            matches!(
                &event.payload,
                EventPayload::TurnStarted {
                    source: bt_core::TurnStartSource::InputResume,
                    resumed_from_call_id: Some(call_id),
                    ..
                } if *call_id == ToolCallId::new("call-ask")
            )
        })
        .expect("input resume turn.started");
    assert_eq!(resumed_start.branch_id, child.branch.branch_id);
}

#[tokio::test]
async fn answer_resume_keeps_paused_settings_revision_after_session_model_change() {
    let mock_provider = spawn_mock_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("answer-settings".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let paused_settings_revision_id = created.session.settings_revision_id;
    let paused_turn_id = TurnId::new();
    let ask_call = ToolCall {
        tool_name: "ask".to_owned(),
        call_id: "call-ask".to_owned(),
        arguments: serde_json::json!({
            "question": "Continue?",
            "choices": ["yes", "no"],
        }),
    };
    server
        .runtime
        .append_message(&created.session, &created.branch, Role::User, "need input")
        .expect("append user message");
    server
        .runtime
        .record_turn_started(
            created.session.session_id,
            created.branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            paused_settings_revision_id,
            bt_core::TurnStartSource::UserMessage,
            None,
        )
        .expect("paused turn started");
    server
        .runtime
        .record_tool_call_requested(
            created.session.session_id,
            created.branch.branch_id,
            ToolCallId::new("call-ask"),
            "ask".to_owned(),
            ask_call.arguments.clone(),
            Some(paused_turn_id),
        )
        .expect("tool call requested");
    server
        .runtime
        .append_raw_message(
            &created.session,
            &created.branch,
            Message::from_part(Role::Assistant, MessagePart::ToolCall { call: ask_call }),
            Some(paused_turn_id),
        )
        .expect("append ask tool call");
    server
        .runtime
        .record_turn_finished(
            created.session.session_id,
            created.branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            "awaiting_input".to_owned(),
            Some("ToolCalls".to_owned()),
            1,
        )
        .expect("finish paused turn");

    let updated: CreateSessionResponse = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}", created.session.session_id),
        )
        .json(&UpdateSessionRequest {
            connection_id: None,
            model_id: Some("updated-model".to_owned()),
            tool_mode: None,
            reset_model_to_default: false,
        })
        .send()
        .await
        .expect("update session")
        .json()
        .await
        .expect("update session body");
    assert_eq!(
        updated.session.settings_revision_id,
        paused_settings_revision_id + 1
    );

    let answer = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/answer", created.session.session_id),
        )
        .json(&AnswerToolRequest {
            call_id: ToolCallId::new("call-ask"),
            response: Value::String("yes".to_owned()),
        })
        .send()
        .await
        .expect("answer tool");
    assert_eq!(answer.status(), StatusCode::ACCEPTED);

    let events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("events request")
        .json()
        .await
        .expect("events body");
    let resumed_start = events
        .events
        .iter()
        .find(|event| {
            matches!(
                &event.payload,
                EventPayload::TurnStarted {
                    source: bt_core::TurnStartSource::InputResume,
                    resumed_from_call_id: Some(call_id),
                    ..
                } if *call_id == ToolCallId::new("call-ask")
            )
        })
        .expect("input resume turn.started");
    assert!(matches!(
        &resumed_start.payload,
        EventPayload::TurnStarted {
            settings_revision_id,
            ..
        } if *settings_revision_id == paused_settings_revision_id
    ));
}

#[tokio::test]
async fn busy_session_send_returns_queued_outcome() {
    let mock_provider = spawn_mock_interrupt_provider(150, &["initial"]).await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("queue-demo".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let send_url = server.url(&format!("sessions/{}/message", created.session.session_id));
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let mut handles = ["start working", "follow up"]
        .into_iter()
        .map(|message| {
            let barrier = Arc::clone(&barrier);
            let send_client = client.clone();
            let send_url = send_url.clone();
            let token = server.token.clone();
            let request = SendMessageRequest {
                branch_id: created.branch.branch_id,
                message: Message::text(Role::User, message),
            };
            tokio::spawn(async move {
                barrier.wait().await;
                let response = send_client
                    .post(send_url)
                    .bearer_auth(token)
                    .header(PROTOCOL_HEADER, PROTOCOL_VERSION)
                    .json(&request)
                    .send()
                    .await
                    .expect("simultaneous send request");
                let status = response.status();
                let body = response
                    .json::<SendMessageResponse>()
                    .await
                    .expect("simultaneous send body");
                (status, body)
            })
        })
        .collect::<Vec<_>>();
    barrier.wait().await;
    let second = handles.pop().expect("second handle").await.expect("join");
    let first = handles.pop().expect("first handle").await.expect("join");
    let responses = [first, second];
    assert!(
        responses
            .iter()
            .all(|(status, response)| *status == StatusCode::ACCEPTED
                && response.session_id == created.session.session_id
                && response.branch_id == created.branch.branch_id)
    );
    assert_eq!(
        responses
            .iter()
            .filter(|(_, response)| response.outcome == SendMessageOutcome::Dispatched)
            .count(),
        1
    );
    assert_eq!(
        responses
            .iter()
            .filter(|(_, response)| {
                response.outcome == SendMessageOutcome::Queued { position: 1 }
            })
            .count(),
        1
    );

    let requests = mock_provider.requests.as_ref().expect("requests capture");
    for _ in 0..40 {
        if requests.lock().expect("requests lock").len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(requests.lock().expect("requests lock").len(), 2);

    let events: SessionEventsResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/events", created.session.session_id),
        )
        .send()
        .await
        .expect("events request")
        .json()
        .await
        .expect("events body");
    let queued_turn_starts = events
        .events
        .iter()
        .filter(|event| {
            matches!(
                event.payload,
                EventPayload::TurnStarted {
                    source: TurnStartSource::QueuedFollowUp,
                    ..
                }
            )
        })
        .count();
    assert_eq!(queued_turn_starts, 1);
}

#[tokio::test]
async fn idle_cancel_request_does_not_queue_next_direct_message() {
    let mock_provider = spawn_mock_provider().await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("idle-cancel".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let cancel = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/cancel", created.session.session_id),
        )
        .json(&CancelSessionRequest {
            reason: Some("cancel before follow-up".to_owned()),
        })
        .send()
        .await
        .expect("cancel session");
    assert_eq!(cancel.status(), StatusCode::ACCEPTED);

    let send = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "continue normally"),
        })
        .send()
        .await
        .expect("send follow-up");
    assert_eq!(send.status(), StatusCode::ACCEPTED);
    let send: SendMessageResponse = send.json().await.expect("send body");
    assert_eq!(send.outcome, SendMessageOutcome::Dispatched);

    let queue: SessionQueueResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/queue", created.session.session_id),
        )
        .send()
        .await
        .expect("queue request")
        .json()
        .await
        .expect("queue body");
    assert!(queue.inspection.queued_messages.is_empty());
    assert!(!queue.inspection.cancel_requested);
}

#[tokio::test]
async fn steer_during_in_flight_turn_runs_a_follow_up_turn() {
    let mock_provider = spawn_mock_interrupt_provider(150, &["initial", "redirected"]).await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("steer-demo".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let send = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "start working"),
        })
        .send()
        .await
        .expect("send message request");
    assert_eq!(send.status(), StatusCode::ACCEPTED);

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let steer = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/steer", created.session.session_id),
        )
        .json(&SteerSessionRequest {
            message: "answer in one sentence".to_owned(),
        })
        .send()
        .await
        .expect("steer request");
    assert_eq!(steer.status(), StatusCode::ACCEPTED);

    settle(&api_client(&server), created.session.session_id).await;

    let messages: SessionMessagesResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/messages", created.session.session_id),
        )
        .send()
        .await
        .expect("messages request")
        .json()
        .await
        .expect("messages body");
    assert!(messages.messages.iter().any(|message| {
        matches!(
            message.parts.as_slice(),
            [MessagePart::Text { text }]
                if message.role == Role::Assistant && text == "initial"
        )
    }));
    assert!(messages.messages.iter().any(|message| {
        matches!(
            message.parts.as_slice(),
            [MessagePart::Text { text }]
                if message.role == Role::User && text == "answer in one sentence"
        )
    }));
    assert!(messages.messages.iter().any(|message| {
        matches!(
            message.parts.as_slice(),
            [MessagePart::Text { text }]
                if message.role == Role::Assistant && text == "redirected"
        )
    }));

    let requests = mock_provider
        .requests
        .as_ref()
        .expect("captured requests")
        .lock()
        .expect("requests lock");
    assert_eq!(requests.len(), 2);
    let second_messages = requests[1]["messages"]
        .as_array()
        .expect("second request messages");
    assert!(second_messages.iter().any(|message| {
        message["role"] == "user"
            && (message["content"] == "answer in one sentence"
                || (message["content"].is_array()
                    && message["content"][0]["text"] == "answer in one sentence"))
    }));
}

#[tokio::test]
async fn cancel_during_in_flight_turn_drops_pending_steer_follow_up() {
    let mock_provider = spawn_mock_interrupt_provider(150, &["initial", "after-cancel"]).await;
    let server = spawn_server(config_for_mock_provider(&mock_provider.base_url)).await;
    let client = Client::new();

    let created: CreateSessionResponse = server
        .request(&client, Method::POST, "sessions")
        .json(&CreateSessionRequest {
            approval_mode: None,
            project_root: "/tmp/project".to_owned(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            tool_mode: None,
            display_name: Some("cancel-demo".to_owned()),
            objective: None,
            budget: None,
        })
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session body");

    let send = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "start working"),
        })
        .send()
        .await
        .expect("send message request");
    assert_eq!(send.status(), StatusCode::ACCEPTED);

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let steer = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/steer", created.session.session_id),
        )
        .json(&SteerSessionRequest {
            message: "this should be dropped".to_owned(),
        })
        .send()
        .await
        .expect("steer request");
    assert_eq!(steer.status(), StatusCode::ACCEPTED);

    let cancel = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/cancel", created.session.session_id),
        )
        .json(&bt_protocol::CancelSessionRequest {
            reason: Some("stop".to_owned()),
        })
        .send()
        .await
        .expect("cancel request");
    assert_eq!(cancel.status(), StatusCode::ACCEPTED);

    let api = api_client(&server);
    settle(&api, created.session.session_id).await;

    {
        let requests = mock_provider
            .requests
            .as_ref()
            .expect("captured requests")
            .lock()
            .expect("requests lock");
        assert_eq!(requests.len(), 1);
    }

    let messages: SessionMessagesResponse = server
        .request(
            &client,
            Method::GET,
            &format!("sessions/{}/messages", created.session.session_id),
        )
        .send()
        .await
        .expect("messages request")
        .json()
        .await
        .expect("messages body");
    assert!(!messages.messages.iter().any(|message| {
        matches!(
            message.parts.as_slice(),
            [MessagePart::Text { text }]
                if message.role == Role::User && text == "this should be dropped"
        )
    }));

    let follow_up = server
        .request(
            &client,
            Method::POST,
            &format!("sessions/{}/message", created.session.session_id),
        )
        .json(&SendMessageRequest {
            branch_id: created.branch.branch_id,
            message: Message::text(Role::User, "fresh turn"),
        })
        .send()
        .await
        .expect("follow-up send");
    assert_eq!(follow_up.status(), StatusCode::ACCEPTED);
    settle(&api, created.session.session_id).await;

    let requests = mock_provider
        .requests
        .as_ref()
        .expect("captured requests")
        .lock()
        .expect("requests lock");
    assert_eq!(requests.len(), 2);
}

async fn spawn_server(config: BelltowerConfig) -> TestServer {
    let temp_dir = TempDir::new().expect("tempdir");
    let database_path = temp_dir.path().join("belltower.sqlite");
    spawn_server_with_database_path(config, &database_path, Some(temp_dir)).await
}

async fn spawn_server_with_database_path(
    config: BelltowerConfig,
    database_path: &std::path::Path,
    temp_dir: Option<TempDir>,
) -> TestServer {
    let runtime =
        Arc::new(BelltowerRuntime::open(config, database_path).expect("runtime should open"));
    let token = "test-token".to_owned();
    let state = AppState {
        runtime: runtime.clone(),
        token: Arc::new(token.clone()),
    };
    let app = build_app(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("server should run");
    });

    TestServer {
        base_url: format!("http://{addr}/"),
        token,
        runtime,
        _temp_dir: temp_dir,
        handle,
    }
}

fn config_for_mock_provider(base_url: &str) -> BelltowerConfig {
    let mut config = BelltowerConfig::from_embedded().expect("config");
    let connection = config
        .connections
        .iter_mut()
        .find(|connection| connection.id == ConnectionId::new("local"))
        .expect("local connection should exist");
    connection.base_url = reqwest::Url::parse(base_url).expect("mock provider url");
    connection.default_model = "o4-mini".to_owned();
    connection.auth_sources.clear();
    config
}

async fn spawn_mock_provider() -> MockProvider {
    async fn models() -> &'static str {
        "{\"data\":[]}"
    }

    async fn completions() -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
        let events = vec![
            Event::default().data(
                "{\"choices\":[{\"delta\":{\"content\":\"hel\"},\"finish_reason\":null}]}",
            ),
            Event::default().data(
                "{\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":null}]}",
            ),
            Event::default().data(
                "{\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":7,\"total_tokens\":19}}",
            ),
            Event::default().data("[DONE]"),
        ];
        Sse::new(futures_util::stream::iter(
            events.into_iter().map(Ok::<_, Infallible>),
        ))
    }

    let app = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(completions));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock should run");
    });

    MockProvider {
        base_url: format!("http://{addr}/v1/"),
        handle,
        requests: None,
    }
}

async fn spawn_mock_broken_stream_provider() -> MockProvider {
    async fn models() -> &'static str {
        "{\"data\":[]}"
    }

    async fn completions() -> axum::response::Response {
        let stream = futures_util::stream::iter(vec![
            Ok::<_, std::io::Error>(axum::body::Bytes::from_static(
                br#"data: {"choices":[{"delta":{"content":"hel"},"finish_reason":null}]}

"#,
            )),
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "mock stream failure",
            )),
        ]);
        axum::response::Response::builder()
            .status(StatusCode::OK)
            .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
            .body(axum::body::Body::from_stream(stream))
            .expect("streaming response")
    }

    let app = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(completions));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock should run");
    });

    MockProvider {
        base_url: format!("http://{addr}/v1/"),
        handle,
        requests: None,
    }
}

async fn spawn_mock_live_stream_provider() -> MockProvider {
    async fn models() -> &'static str {
        "{\"data\":[]}"
    }

    async fn completions() -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
        let stream = async_stream::stream! {
            yield Ok::<_, Infallible>(Event::default().data(
                "{\"choices\":[{\"delta\":{\"content\":\"hel\"},\"finish_reason\":null}]}"
            ));
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            yield Ok::<_, Infallible>(Event::default().data(
                "{\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":null}]}"
            ));
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            yield Ok::<_, Infallible>(Event::default().data("[DONE]"));
        };

        Sse::new(stream)
    }

    let app = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(completions));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock should run");
    });

    MockProvider {
        base_url: format!("http://{addr}/v1/"),
        handle,
        requests: None,
    }
}

async fn spawn_controlled_stream_provider() -> ControlledStreamProvider {
    async fn models() -> &'static str {
        "{\"data\":[]}"
    }

    async fn completions(
        State(state): State<ControlledStreamProviderState>,
    ) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
        let _ = state.request_seen.send(true);
        let mut release_stream = state.release_stream.subscribe();
        let stream = async_stream::stream! {
            while !*release_stream.borrow_and_update() {
                if release_stream.changed().await.is_err() {
                    return;
                }
            }
            yield Ok::<_, Infallible>(Event::default().data(
                "{\"choices\":[{\"delta\":{\"content\":\"blocked\"},\"finish_reason\":null}]}"
            ));
            yield Ok::<_, Infallible>(Event::default().data("[DONE]"));
        };

        Sse::new(stream)
    }

    let (request_seen_tx, request_seen_rx) = tokio::sync::watch::channel(false);
    let (release_stream_tx, _) = tokio::sync::watch::channel(false);
    let state = ControlledStreamProviderState {
        request_seen: request_seen_tx,
        release_stream: release_stream_tx.clone(),
    };
    let app = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(completions))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock should run");
    });

    ControlledStreamProvider {
        base_url: format!("http://{addr}/v1/"),
        request_seen: request_seen_rx,
        release_stream: release_stream_tx,
        handle,
    }
}

async fn spawn_mock_otlp_collector() -> MockOtlpCollector {
    spawn_mock_otlp_collector_with_response(MockOtlpCollectorResponse::empty_ok()).await
}

async fn spawn_mock_otlp_collector_with_response(
    response: MockOtlpCollectorResponse,
) -> MockOtlpCollector {
    async fn receive(
        State((captured, response)): State<(
            Arc<Mutex<Option<CapturedOtlpRequest>>>,
            MockOtlpCollectorResponse,
        )>,
        headers: axum::http::HeaderMap,
        body: axum::body::Bytes,
    ) -> axum::response::Response {
        let mut header_map = BTreeMap::new();
        for (name, value) in &headers {
            header_map.insert(
                name.as_str().to_owned(),
                value.to_str().unwrap_or_default().to_owned(),
            );
        }
        let request = CapturedOtlpRequest {
            headers: header_map,
            body: body.to_vec(),
        };
        *captured.lock().expect("collector lock") = Some(request);

        let mut response_builder = axum::response::Response::builder().status(response.status);
        if let Some(content_type) = &response.content_type {
            response_builder =
                response_builder.header(axum::http::header::CONTENT_TYPE, content_type);
        }
        response_builder
            .body(axum::body::Body::from(response.body.clone()))
            .expect("collector response")
    }

    let captured = Arc::new(Mutex::new(None));
    let app = Router::new()
        .route("/v1/traces", post(receive))
        .with_state((captured.clone(), response));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind collector");
    let addr = listener.local_addr().expect("collector addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("collector should run");
    });

    MockOtlpCollector {
        base_url: format!("http://{addr}"),
        captured,
        handle,
    }
}

async fn spawn_mock_approval_provider() -> MockProvider {
    async fn models() -> &'static str {
        "{\"data\":[]}"
    }

    async fn completions(
        axum::extract::State(state): axum::extract::State<MockApprovalProviderState>,
        Json(payload): Json<Value>,
    ) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
        state.requests.lock().expect("requests lock").push(payload);
        let request_index = state.counter.fetch_add(1, Ordering::SeqCst);
        let events = if request_index == 0 {
            vec![
                Event::default().data(
                    "{\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-approval\",\"function\":{\"name\":\"shell\",\"arguments\":\"{\\\"command\\\":\\\"printf approved\\\",\\\"call_id\\\":\\\"model-call-id\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}",
                ),
                Event::default().data("[DONE]"),
            ]
        } else {
            vec![
                Event::default().data(
                    "{\"choices\":[{\"delta\":{\"content\":\"complete\"},\"finish_reason\":null}]}",
                ),
                Event::default().data("[DONE]"),
            ]
        };

        Sse::new(futures_util::stream::iter(
            events.into_iter().map(Ok::<_, Infallible>),
        ))
    }

    let state = MockApprovalProviderState {
        counter: Arc::new(AtomicUsize::new(0)),
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(completions))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock should run");
    });

    MockProvider {
        base_url: format!("http://{addr}/v1/"),
        handle,
        requests: Some(state.requests),
    }
}

async fn spawn_mock_repeated_approval_provider() -> MockProvider {
    async fn models() -> &'static str {
        "{\"data\":[]}"
    }

    async fn completions(
        axum::extract::State(state): axum::extract::State<MockApprovalProviderState>,
        Json(payload): Json<Value>,
    ) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
        state.requests.lock().expect("requests lock").push(payload);
        let request_index = state.counter.fetch_add(1, Ordering::SeqCst);
        let events = if request_index < 2 {
            vec![
                Event::default().data(
                    "{\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-approval-repeat\",\"function\":{\"name\":\"shell\",\"arguments\":\"{\\\"command\\\":\\\"printf approved\\\",\\\"call_id\\\":\\\"model-call-id\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}",
                ),
                Event::default().data("[DONE]"),
            ]
        } else {
            vec![
                Event::default().data(
                    "{\"choices\":[{\"delta\":{\"content\":\"complete\"},\"finish_reason\":null}]}",
                ),
                Event::default().data("[DONE]"),
            ]
        };

        Sse::new(futures_util::stream::iter(
            events.into_iter().map(Ok::<_, Infallible>),
        ))
    }

    let state = MockApprovalProviderState {
        counter: Arc::new(AtomicUsize::new(0)),
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(completions))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock should run");
    });

    MockProvider {
        base_url: format!("http://{addr}/v1/"),
        handle,
        requests: Some(state.requests),
    }
}

async fn spawn_mock_dual_approval_provider() -> MockProvider {
    async fn models() -> &'static str {
        "{\"data\":[]}"
    }

    async fn completions(
        axum::extract::State(state): axum::extract::State<MockApprovalProviderState>,
        Json(payload): Json<Value>,
    ) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
        state.requests.lock().expect("requests lock").push(payload);
        let request_index = state.counter.fetch_add(1, Ordering::SeqCst);
        let events = if request_index == 0 {
            vec![
                Event::default().data(
                    "{\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-approval-one\",\"function\":{\"name\":\"shell\",\"arguments\":\"{\\\"command\\\":\\\"printf first\\\",\\\"call_id\\\":\\\"model-call-id-one\\\"}\"}},{\"index\":1,\"id\":\"call-approval-two\",\"function\":{\"name\":\"shell\",\"arguments\":\"{\\\"command\\\":\\\"printf second\\\",\\\"call_id\\\":\\\"model-call-id-two\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}",
                ),
                Event::default().data("[DONE]"),
            ]
        } else {
            vec![
                Event::default().data(
                    "{\"choices\":[{\"delta\":{\"content\":\"complete\"},\"finish_reason\":null}]}",
                ),
                Event::default().data("[DONE]"),
            ]
        };

        Sse::new(futures_util::stream::iter(
            events.into_iter().map(Ok::<_, Infallible>),
        ))
    }

    let state = MockApprovalProviderState {
        counter: Arc::new(AtomicUsize::new(0)),
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(completions))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock should run");
    });

    MockProvider {
        base_url: format!("http://{addr}/v1/"),
        handle,
        requests: Some(state.requests),
    }
}

#[derive(Clone)]
struct MockApprovalProviderState {
    counter: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Value>>>,
}

async fn spawn_mock_safe_tool_provider() -> MockProvider {
    async fn models() -> &'static str {
        "{\"data\":[]}"
    }

    async fn completions(
        axum::extract::State(state): axum::extract::State<MockInspectionProviderState>,
        Json(payload): Json<Value>,
    ) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
        state.requests.lock().expect("requests lock").push(payload);
        let request_index = state.counter.fetch_add(1, Ordering::SeqCst);
        let events = if request_index == 0 {
            vec![
                Event::default().data(
                    "{\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-list-same-turn\",\"function\":{\"name\":\"list\",\"arguments\":\"{\\\"path\\\":\\\".\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}",
                ),
                Event::default().data("[DONE]"),
            ]
        } else {
            vec![
                Event::default().data(
                    "{\"choices\":[{\"delta\":{\"content\":\"listed\"},\"finish_reason\":null}]}",
                ),
                Event::default().data(
                    "{\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":2,\"total_tokens\":14}}",
                ),
                Event::default().data("[DONE]"),
            ]
        };

        Sse::new(futures_util::stream::iter(
            events.into_iter().map(Ok::<_, Infallible>),
        ))
    }

    let state = MockInspectionProviderState {
        counter: Arc::new(AtomicUsize::new(0)),
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(completions))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock should run");
    });

    MockProvider {
        base_url: format!("http://{addr}/v1/"),
        handle,
        requests: Some(state.requests),
    }
}

async fn spawn_mock_inspection_contract_provider() -> MockProvider {
    async fn models() -> &'static str {
        "{\"data\":[]}"
    }

    async fn completions(
        axum::extract::State(state): axum::extract::State<MockInspectionProviderState>,
        Json(payload): Json<Value>,
    ) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
        state.requests.lock().expect("requests lock").push(payload);
        let request_index = state.counter.fetch_add(1, Ordering::SeqCst);
        let events = match request_index {
            0 => vec![
                Event::default().data(
                    "{\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-shell-approval\",\"function\":{\"name\":\"shell\",\"arguments\":\"{\\\"command\\\":\\\"printf approved\\\",\\\"call_id\\\":\\\"model-call-id\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}",
                ),
                Event::default().data("[DONE]"),
            ],
            1 => vec![
                Event::default().data(
                    "{\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-mcp-approval\",\"function\":{\"name\":\"mcp_fixture_echo_remote\",\"arguments\":\"{\\\"text\\\":\\\"remote hello\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}",
                ),
                Event::default().data("[DONE]"),
            ],
            _ => vec![
                Event::default().data(
                    "{\"choices\":[{\"delta\":{\"content\":\"inspection contract complete\"},\"finish_reason\":null}]}",
                ),
                Event::default().data(
                    "{\"choices\":[],\"usage\":{\"prompt_tokens\":21,\"completion_tokens\":8,\"total_tokens\":29}}",
                ),
                Event::default().data("[DONE]"),
            ],
        };

        Sse::new(futures_util::stream::iter(
            events.into_iter().map(Ok::<_, Infallible>),
        ))
    }

    let state = MockInspectionProviderState {
        counter: Arc::new(AtomicUsize::new(0)),
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(completions))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock should run");
    });

    MockProvider {
        base_url: format!("http://{addr}/v1/"),
        handle,
        requests: Some(state.requests),
    }
}

#[derive(Clone)]
struct MockInspectionProviderState {
    counter: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Value>>>,
}

struct MockHttpMcpServer {
    base_url: String,
    handle: JoinHandle<()>,
}

impl Drop for MockHttpMcpServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn spawn_mock_http_mcp_server() -> MockHttpMcpServer {
    async fn rpc(Json(request): Json<Value>) -> Json<Value> {
        let id = request.get("id").cloned();
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let result = match method {
            "initialize" => json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "fake-http-mcp", "version": "0.1.0" }
            }),
            "tools/list" => json!({
                "tools": [{
                    "name": "echo_remote",
                    "description": "Echoes text from HTTP MCP",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "text": { "type": "string" }
                        },
                        "required": ["text"],
                        "additionalProperties": false
                    }
                }]
            }),
            "tools/call" => {
                let text = request["params"]["arguments"]["text"]
                    .as_str()
                    .unwrap_or_default();
                json!({
                    "content": [{
                        "type": "text",
                        "text": format!("echo: {text}")
                    }],
                    "structuredContent": {
                        "echoed": text
                    },
                    "isError": false
                })
            }
            _ => json!({}),
        };
        Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }))
    }

    let app = Router::new().route("/", post(rpc));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock http mcp");
    let addr = listener.local_addr().expect("mock http mcp addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("mock http mcp should run");
    });

    MockHttpMcpServer {
        base_url: format!("http://{addr}/"),
        handle,
    }
}

async fn spawn_mock_interrupt_provider(delay_ms: u64, responses: &[&str]) -> MockProvider {
    async fn models() -> &'static str {
        "{\"data\":[]}"
    }

    async fn completions(
        axum::extract::State(state): axum::extract::State<MockInterruptProviderState>,
        Json(payload): Json<Value>,
    ) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
        state.requests.lock().expect("requests lock").push(payload);
        let request_index = state.counter.fetch_add(1, Ordering::SeqCst);
        if request_index == 0 && state.delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(state.delay_ms)).await;
        }
        let text = state
            .responses
            .get(request_index)
            .cloned()
            .unwrap_or_else(|| format!("response-{request_index}"));
        let events = vec![
            Event::default().data(format!(
                "{{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}},\"finish_reason\":null}}]}}"
            )),
            Event::default().data("[DONE]"),
        ];

        Sse::new(futures_util::stream::iter(
            events.into_iter().map(Ok::<_, Infallible>),
        ))
    }

    let state = MockInterruptProviderState {
        counter: Arc::new(AtomicUsize::new(0)),
        requests: Arc::new(Mutex::new(Vec::new())),
        delay_ms,
        responses: responses.iter().map(|value| (*value).to_owned()).collect(),
    };
    let app = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(completions))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock should run");
    });

    MockProvider {
        base_url: format!("http://{addr}/v1/"),
        handle,
        requests: Some(state.requests),
    }
}

#[derive(Clone)]
struct MockInterruptProviderState {
    counter: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Value>>>,
    delay_ms: u64,
    responses: Vec<String>,
}
