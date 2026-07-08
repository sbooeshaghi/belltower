#![forbid(unsafe_code)]

use async_stream::try_stream;
use bt_core::{
    BelltowerError, BranchId, ConnectionId, ErrorClass, SessionId, ToolCallId, TurnId,
    auth_token_path, server_auth_token_path_for_url,
};
use bt_protocol::{
    ActivateBranchRequest, AnswerToolRequest, ApproveToolRequest, BranchMessagesPageResponse,
    BranchOperatorCommandsPageResponse, BranchOperatorCommandsResponse, BranchesResponse,
    CancelSessionRequest, CompactSessionRequest, CompactSessionResponse, ConnectionModelsResponse,
    ConnectionsResponse, CreateBranchRequest, CreateBranchResponse, CreateSessionRequest,
    CreateSessionResponse, ErrorEnvelope, HealthResponse, ListSessionsResponse,
    McpInventoryResponse, McpServersResponse, McpToolsResponse, ModelBackendsResponse,
    ModelRecommendationsResponse, PROTOCOL_HEADER, PROTOCOL_VERSION, PushOtlpExportRequest,
    PushOtlpExportResponse, RawSseEnvelope, RecordOperatorCommandRequest, RunShellCommandRequest,
    SendMessageRequest, SendMessageResponse, ServerInfoResponse, SessionBranchInspectionResponse,
    SessionEventsResponse, SessionExecutionResponse, SessionExportResponse,
    SessionInspectionResponse, SessionLineageResponse, SessionMessagesResponse,
    SessionQueueClearResponse, SessionQueueResponse, SessionRawChunksResponse,
    SessionSearchResponse, SessionToolCallResponse, SessionTreeResponse, SessionTurnsResponse,
    SessionWorkflowResponse, SpawnSessionRequest, SpawnSessionResponse, StatusInspectionResponse,
    SteerSessionRequest, TurnRawChunksPageResponse, UpdateSessionBudgetRequest,
    UpdateSessionRequest,
};
use futures_core::Stream;
use futures_util::StreamExt;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use reqwest::{Client as HttpClient, Method, RequestBuilder, Response, StatusCode};
use std::fs;
use std::pin::Pin;
use url::Url;

#[derive(Debug)]
pub enum ClientError {
    Core(BelltowerError),
    Http(reqwest::Error),
    Api {
        status: StatusCode,
        class: ErrorClass,
        code: Option<String>,
        message: String,
        retryable: bool,
    },
    Io(std::io::Error),
    InvalidAuthHeader,
}

impl ClientError {
    #[must_use]
    pub const fn class(&self) -> Option<ErrorClass> {
        match self {
            Self::Core(error) => Some(error.class()),
            Self::Api { class, .. } => Some(*class),
            Self::Http(_) | Self::Io(_) | Self::InvalidAuthHeader => None,
        }
    }

    #[must_use]
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Core(error) => Some(error.code()),
            Self::Api { code, .. } => code.as_deref(),
            Self::Http(_) | Self::Io(_) | Self::InvalidAuthHeader => None,
        }
    }

    #[must_use]
    pub const fn retryable(&self) -> Option<bool> {
        match self {
            Self::Core(error) => Some(error.retryable()),
            Self::Api { retryable, .. } => Some(*retryable),
            Self::Http(_) => Some(true),
            Self::Io(_) | Self::InvalidAuthHeader => None,
        }
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Core(error) => write!(
                f,
                "core error [{}:{}]: {}",
                error.class(),
                error.code(),
                error
            ),
            Self::Http(error) => write!(f, "http error: {error}"),
            Self::Api {
                status,
                class,
                code,
                message,
                retryable,
            } => {
                write!(
                    f,
                    "api error ({status} {class}:{}): {message}",
                    code.as_deref().unwrap_or("unknown_error")
                )?;
                if *retryable {
                    write!(f, " [retryable]")?;
                }
                Ok(())
            }
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::InvalidAuthHeader => f.write_str("invalid authorization header"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<BelltowerError> for ClientError {
    fn from(value: BelltowerError) -> Self {
        Self::Core(value)
    }
}

impl From<reqwest::Error> for ClientError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

impl From<std::io::Error> for ClientError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub type Result<T> = std::result::Result<T, ClientError>;
pub type EventStream = Pin<Box<dyn Stream<Item = Result<RawSseEnvelope>> + Send>>;

#[derive(Clone, Debug)]
pub struct BelltowerClient {
    http: HttpClient,
    base_url: Url,
    auth_source: ClientAuthSource,
}

#[derive(Clone, Debug)]
enum ClientAuthSource {
    DiscoverFromBaseUrl,
    Path(std::path::PathBuf),
    Static(String),
}

impl BelltowerClient {
    /// # Launcher / local TUI only
    #[deprecated(
        note = "BelltowerClient::new performs launcher-style auth discovery. Use new_with_auth_token for remote clients or new_discovering_auth for launcher-owned local bootstrap."
    )]
    pub fn new(base_url: Url) -> Result<Self> {
        Self::new_discovering_auth(base_url)
    }

    /// # Launcher / local TUI only
    pub fn new_discovering_auth(base_url: Url) -> Result<Self> {
        Ok(Self {
            http: HttpClient::builder().build()?,
            base_url,
            auth_source: ClientAuthSource::DiscoverFromBaseUrl,
        })
    }

    /// # Remote client-safe
    pub fn new_with_auth_token(base_url: Url, token: impl Into<String>) -> Result<Self> {
        Ok(Self::new_discovering_auth(base_url)?.with_auth_token(token))
    }

    /// # Launcher / local TUI only
    pub fn new_with_auth_token_path(
        base_url: Url,
        path: impl Into<std::path::PathBuf>,
    ) -> Result<Self> {
        Ok(Self::new_discovering_auth(base_url)?.with_auth_token_path(path))
    }

    #[must_use]
    /// # Launcher / local TUI only
    pub fn with_auth_token_path(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.auth_source = ClientAuthSource::Path(path.into());
        self
    }

    #[must_use]
    /// # Remote client-safe
    pub fn with_auth_token(mut self, token: impl Into<String>) -> Self {
        self.auth_source = ClientAuthSource::Static(token.into());
        self
    }

    /// # Remote client-safe
    pub async fn health(&self) -> Result<HealthResponse> {
        Ok(self
            .send_checked(self.request_without_auth(Method::GET, "health")?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn server_info(&self) -> Result<ServerInfoResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, "server/info")?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn create_session(
        &self,
        request: &CreateSessionRequest,
    ) -> Result<CreateSessionResponse> {
        Ok(self
            .send_checked(self.request(Method::POST, "sessions")?.json(request))
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn spawn_session(
        &self,
        session_id: SessionId,
        request: &SpawnSessionRequest,
    ) -> Result<SpawnSessionResponse> {
        Ok(self
            .send_checked(
                self.request(Method::POST, &format!("sessions/{session_id}/spawn"))?
                    .json(request),
            )
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn list_sessions(&self) -> Result<ListSessionsResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, "sessions")?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn inspect_session(
        &self,
        session_id: SessionId,
    ) -> Result<SessionInspectionResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, &format!("sessions/{session_id}"))?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn update_session(
        &self,
        session_id: SessionId,
        request: &UpdateSessionRequest,
    ) -> Result<CreateSessionResponse> {
        Ok(self
            .send_checked(
                self.request(Method::POST, &format!("sessions/{session_id}"))?
                    .json(request),
            )
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn update_session_budget(
        &self,
        session_id: SessionId,
        request: &UpdateSessionBudgetRequest,
    ) -> Result<SessionInspectionResponse> {
        Ok(self
            .send_checked(
                self.request(Method::POST, &format!("sessions/{session_id}/budget"))?
                    .json(request),
            )
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn send_message(
        &self,
        session_id: SessionId,
        request: &SendMessageRequest,
    ) -> Result<SendMessageResponse> {
        Ok(self
            .send_checked(
                self.request(Method::POST, &format!("sessions/{session_id}/message"))?
                    .json(request),
            )
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn create_branch(
        &self,
        session_id: SessionId,
        request: &CreateBranchRequest,
    ) -> Result<CreateBranchResponse> {
        Ok(self
            .send_checked(
                self.request(Method::POST, &format!("sessions/{session_id}/branches"))?
                    .json(request),
            )
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn compact_session(
        &self,
        session_id: SessionId,
        request: &CompactSessionRequest,
    ) -> Result<CompactSessionResponse> {
        Ok(self
            .send_checked(
                self.request(Method::POST, &format!("sessions/{session_id}/compact"))?
                    .json(request),
            )
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn activate_branch(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        request: &ActivateBranchRequest,
    ) -> Result<Response> {
        self.send_checked(
            self.request(
                Method::POST,
                &format!("sessions/{session_id}/branches/{branch_id}/activate"),
            )?
            .json(request),
        )
        .await
    }

    /// # Remote client-safe
    pub async fn approve_tool(
        &self,
        session_id: SessionId,
        request: &ApproveToolRequest,
    ) -> Result<Response> {
        self.send_checked(
            self.request(Method::POST, &format!("sessions/{session_id}/approve"))?
                .json(request),
        )
        .await
    }

    /// # Remote client-safe
    pub async fn answer_tool(
        &self,
        session_id: SessionId,
        request: &AnswerToolRequest,
    ) -> Result<Response> {
        self.send_checked(
            self.request(Method::POST, &format!("sessions/{session_id}/answer"))?
                .json(request),
        )
        .await
    }

    /// # Remote client-safe
    pub async fn cancel_session(
        &self,
        session_id: SessionId,
        request: &CancelSessionRequest,
    ) -> Result<Response> {
        self.send_checked(
            self.request(Method::POST, &format!("sessions/{session_id}/cancel"))?
                .json(request),
        )
        .await
    }

    /// # Remote client-safe
    pub async fn steer_session(
        &self,
        session_id: SessionId,
        request: &SteerSessionRequest,
    ) -> Result<Response> {
        self.send_checked(
            self.request(Method::POST, &format!("sessions/{session_id}/steer"))?
                .json(request),
        )
        .await
    }

    /// # Remote client-safe
    pub async fn record_operator_command(
        &self,
        session_id: SessionId,
        request: &RecordOperatorCommandRequest,
    ) -> Result<Response> {
        self.send_checked(
            self.request(Method::POST, &format!("sessions/{session_id}/commands"))?
                .json(request),
        )
        .await
    }

    /// # Remote client-safe
    pub async fn run_shell_command(
        &self,
        session_id: SessionId,
        request: &RunShellCommandRequest,
    ) -> Result<Response> {
        self.send_checked(
            self.request(
                Method::POST,
                &format!("sessions/{session_id}/commands/shell"),
            )?
            .json(request),
        )
        .await
    }

    /// # Remote client-safe
    pub async fn session_events(
        &self,
        session_id: SessionId,
        last_event_id: Option<i64>,
    ) -> Result<SessionEventsResponse> {
        let mut request = self.request(Method::GET, &format!("sessions/{session_id}/events"))?;
        if let Some(last_event_id) = last_event_id {
            request = request.header("last-event-id", last_event_id.to_string());
        }
        Ok(self.send_checked(request).await?.json().await?)
    }

    /// # Remote client-safe
    pub async fn session_turns(&self, session_id: SessionId) -> Result<SessionTurnsResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, &format!("sessions/{session_id}/turns"))?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn session_execution(
        &self,
        session_id: SessionId,
    ) -> Result<SessionExecutionResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, &format!("sessions/{session_id}/execution"))?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn session_queue(&self, session_id: SessionId) -> Result<SessionQueueResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, &format!("sessions/{session_id}/queue"))?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn clear_session_queue(
        &self,
        session_id: SessionId,
    ) -> Result<SessionQueueClearResponse> {
        Ok(self
            .send_checked(
                self.request(Method::POST, &format!("sessions/{session_id}/queue/clear"))?,
            )
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn session_tool_call(
        &self,
        session_id: SessionId,
        call_id: &ToolCallId,
    ) -> Result<SessionToolCallResponse> {
        Ok(self
            .send_checked(self.request(
                Method::GET,
                &format!("sessions/{session_id}/tool-calls/{call_id}"),
            )?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn search_session(
        &self,
        session_id: SessionId,
        query: &str,
        branch_id: Option<BranchId>,
        limit: Option<usize>,
    ) -> Result<SessionSearchResponse> {
        let mut url = self
            .base_url
            .join(&format!("sessions/{session_id}/search"))
            .map_err(|error| ClientError::Core(BelltowerError::Url(error)))?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("query", query);
            if let Some(branch_id) = branch_id {
                pairs.append_pair("branch_id", &branch_id.to_string());
            }
            if let Some(limit) = limit {
                pairs.append_pair("limit", &limit.to_string());
            }
        }
        Ok(self
            .send_checked(
                self.http
                    .request(Method::GET, url)
                    .headers(self.auth_headers()?),
            )
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn session_lineage(&self, session_id: SessionId) -> Result<SessionLineageResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, &format!("sessions/{session_id}/lineage"))?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn session_workflow(&self, session_id: SessionId) -> Result<SessionWorkflowResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, &format!("sessions/{session_id}/workflow"))?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn inspect_branches(
        &self,
        session_id: SessionId,
    ) -> Result<SessionBranchInspectionResponse> {
        Ok(self
            .send_checked(self.request(
                Method::GET,
                &format!("sessions/{session_id}/branches/inspect"),
            )?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn session_tree(&self, session_id: SessionId) -> Result<SessionTreeResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, &format!("sessions/{session_id}/tree"))?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn stream_events(
        &self,
        session_id: SessionId,
        last_event_id: Option<i64>,
    ) -> Result<EventStream> {
        let mut request =
            self.request(Method::GET, &format!("sessions/{session_id}/events/stream"))?;
        if let Some(last_event_id) = last_event_id {
            request = request.header("last-event-id", last_event_id.to_string());
        }
        let response = self.send_checked(request).await?;

        let mut bytes_stream = response.bytes_stream();
        let stream = try_stream! {
            let mut parser = ClientSseParser::new();
            while let Some(chunk) = bytes_stream.next().await {
                let chunk = chunk?;
                for frame in parser.push(chunk.as_ref()) {
                    let envelope =
                        serde_json::from_str::<RawSseEnvelope>(&frame).map_err(BelltowerError::from)?;
                    yield envelope;
                }
            }
        };

        Ok(Box::pin(stream))
    }

    /// # Remote client-safe
    pub async fn session_messages(&self, session_id: SessionId) -> Result<SessionMessagesResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, &format!("sessions/{session_id}/messages"))?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn branch_messages(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
    ) -> Result<SessionMessagesResponse> {
        Ok(self
            .send_checked(self.request(
                Method::GET,
                &format!("sessions/{session_id}/branches/{branch_id}/messages"),
            )?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn branch_operator_commands(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
    ) -> Result<BranchOperatorCommandsResponse> {
        Ok(self
            .send_checked(self.request(
                Method::GET,
                &format!("sessions/{session_id}/branches/{branch_id}/commands"),
            )?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn branch_messages_page(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        before_seq: Option<i64>,
        limit: usize,
    ) -> Result<BranchMessagesPageResponse> {
        let path = format!(
            "sessions/{session_id}/branches/{branch_id}/messages/page?limit={limit}{}",
            before_seq
                .map(|value| format!("&before_seq={value}"))
                .unwrap_or_default()
        );
        Ok(self
            .send_checked(self.request(Method::GET, &path)?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn branch_operator_commands_page(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        before_seq: Option<i64>,
        limit: usize,
    ) -> Result<BranchOperatorCommandsPageResponse> {
        let path = format!(
            "sessions/{session_id}/branches/{branch_id}/commands/page?limit={limit}{}",
            before_seq
                .map(|value| format!("&before_seq={value}"))
                .unwrap_or_default()
        );
        Ok(self
            .send_checked(self.request(Method::GET, &path)?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn branches(&self, session_id: SessionId) -> Result<BranchesResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, &format!("sessions/{session_id}/branches"))?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn raw_chunks(&self, session_id: SessionId) -> Result<SessionRawChunksResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, &format!("sessions/{session_id}/raw_chunks"))?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn turn_raw_chunks_page(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        turn_id: TurnId,
        llm_call_ordinal: Option<u32>,
        before_chunk_id: Option<i64>,
        limit: usize,
    ) -> Result<TurnRawChunksPageResponse> {
        let path = format!(
            "sessions/{session_id}/branches/{branch_id}/turns/{turn_id}/raw_chunks/page?limit={limit}{}{}",
            llm_call_ordinal
                .map(|value| format!("&llm_call_ordinal={value}"))
                .unwrap_or_default(),
            before_chunk_id
                .map(|value| format!("&before_chunk_id={value}"))
                .unwrap_or_default()
        );
        Ok(self
            .send_checked(self.request(Method::GET, &path)?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn export_session(
        &self,
        session_id: SessionId,
        format: &str,
    ) -> Result<SessionExportResponse> {
        Ok(self
            .send_checked(self.request(
                Method::GET,
                &format!("sessions/{session_id}/export/{format}"),
            )?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn push_otlp_export(
        &self,
        session_id: SessionId,
        request: &PushOtlpExportRequest,
    ) -> Result<PushOtlpExportResponse> {
        Ok(self
            .send_checked(
                self.request(
                    Method::POST,
                    &format!("sessions/{session_id}/export/otlp/push"),
                )?
                .json(request),
            )
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn connections(&self) -> Result<ConnectionsResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, "connections")?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn status_inspection(&self) -> Result<StatusInspectionResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, "status/inspect")?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn mcp_servers(&self) -> Result<McpServersResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, "mcp/servers")?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn mcp_inventory(&self) -> Result<McpInventoryResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, "mcp")?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn mcp_tools(&self) -> Result<McpToolsResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, "mcp/tools")?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn reload_mcp(&self) -> Result<Response> {
        self.send_checked(self.request(Method::POST, "mcp/reload")?)
            .await
    }

    /// # Remote client-safe
    pub async fn model_backends(&self) -> Result<ModelBackendsResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, "models/backends")?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn model_recommendations(&self) -> Result<ModelRecommendationsResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, "models/recommendations")?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn connection_models(&self) -> Result<ConnectionModelsResponse> {
        Ok(self
            .send_checked(self.request(Method::GET, "models/connections")?)
            .await?
            .json()
            .await?)
    }

    /// # Remote client-safe
    pub async fn connection_model_inventory(
        &self,
        connection_id: &bt_core::ConnectionId,
    ) -> Result<ConnectionModelsResponse> {
        Ok(self
            .send_checked(
                self.request(Method::GET, &format!("models/connections/{connection_id}"))?,
            )
            .await?
            .json()
            .await?)
    }

    #[cfg(test)]
    pub(crate) fn stream_headers(&self, last_event_id: Option<i64>) -> Result<HeaderMap> {
        let mut headers = self.auth_headers()?;
        if let Some(last_event_id) = last_event_id {
            headers.insert(
                "last-event-id",
                HeaderValue::from_str(&last_event_id.to_string())
                    .map_err(|_| ClientError::InvalidAuthHeader)?,
            );
        }
        Ok(headers)
    }

    fn request(&self, method: Method, path: &str) -> Result<reqwest::RequestBuilder> {
        let url = self
            .base_url
            .join(path)
            .map_err(|error| ClientError::Core(BelltowerError::Url(error)))?;
        Ok(self.http.request(method, url).headers(self.auth_headers()?))
    }

    fn request_without_auth(&self, method: Method, path: &str) -> Result<reqwest::RequestBuilder> {
        let url = self
            .base_url
            .join(path)
            .map_err(|error| ClientError::Core(BelltowerError::Url(error)))?;
        let mut headers = HeaderMap::new();
        headers.insert(PROTOCOL_HEADER, HeaderValue::from_static(PROTOCOL_VERSION));
        Ok(self.http.request(method, url).headers(headers))
    }

    async fn send_checked(&self, request: RequestBuilder) -> Result<Response> {
        let response = request.send().await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }

        let body = response.text().await.unwrap_or_default();
        if let Ok(envelope) = serde_json::from_str::<ErrorEnvelope>(&body) {
            return Err(ClientError::Api {
                status,
                class: envelope.class,
                code: Some(envelope.code),
                message: envelope.message,
                retryable: envelope.retryable,
            });
        }

        let message = if body.trim().is_empty() {
            format!("request failed with status {status}")
        } else {
            body
        };
        Err(ClientError::Api {
            status,
            class: ErrorClass::Runtime,
            code: None,
            message,
            retryable: status.is_server_error(),
        })
    }

    fn auth_headers(&self) -> Result<HeaderMap> {
        let token = self.auth_token()?;
        let mut headers = HeaderMap::new();
        headers.insert(PROTOCOL_HEADER, HeaderValue::from_static(PROTOCOL_VERSION));
        let value = format!("Bearer {token}");
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&value).map_err(|_| ClientError::InvalidAuthHeader)?,
        );
        Ok(headers)
    }

    fn auth_token(&self) -> Result<String> {
        Ok(match &self.auth_source {
            ClientAuthSource::Static(token) => token.clone(),
            ClientAuthSource::Path(path) => read_auth_token_file(path)?.ok_or_else(|| {
                ClientError::Core(BelltowerError::InvalidState(format!(
                    "auth token file is empty: {}",
                    path.display()
                )))
            })?,
            ClientAuthSource::DiscoverFromBaseUrl => {
                let paths = discovered_auth_token_paths(&self.base_url)?;
                load_auth_token_from_paths(&paths)?
            }
        })
    }
}

fn discovered_auth_token_paths(base_url: &Url) -> Result<Vec<std::path::PathBuf>> {
    let mut paths = Vec::new();
    let endpoint = server_auth_token_path_for_url(base_url)
        .map_err(ClientError::Core)?
        .into_std_path_buf();
    paths.push(endpoint.clone());
    let legacy = auth_token_path().into_std_path_buf();
    if legacy != endpoint {
        paths.push(legacy);
    }
    Ok(paths)
}

fn load_auth_token_from_paths(paths: &[std::path::PathBuf]) -> Result<String> {
    let mut last_error = None;
    for path in paths {
        match read_auth_token_file(path) {
            Ok(Some(token)) => return Ok(token),
            Ok(None) => continue,
            Err(error) => last_error = Some(error),
        }
    }
    Err(
        last_error.unwrap_or(ClientError::Core(BelltowerError::InvalidState(
            "no auth token sources configured".to_owned(),
        ))),
    )
}

fn read_auth_token_file(path: &std::path::Path) -> Result<Option<String>> {
    let token = fs::read_to_string(path)?;
    let token = token.trim().to_owned();
    if token.is_empty() {
        return Ok(None);
    }
    Ok(Some(token))
}

impl TryFrom<&str> for BelltowerClient {
    type Error = ClientError;

    fn try_from(value: &str) -> Result<Self> {
        Self::new_discovering_auth(Url::parse(value).map_err(BelltowerError::Url)?)
    }
}

impl TryFrom<ConnectionId> for BelltowerClient {
    type Error = ClientError;

    fn try_from(_value: ConnectionId) -> Result<Self> {
        Self::try_from("http://127.0.0.1:7400/")
    }
}

#[derive(Default)]
struct ClientSseParser {
    buffer: String,
}

impl ClientSseParser {
    fn new() -> Self {
        Self::default()
    }

    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        self.buffer = self.buffer.replace("\r\n", "\n");
        let mut events = Vec::new();
        while let Some(boundary) = self.buffer.find("\n\n") {
            let frame = self.buffer[..boundary].to_owned();
            self.buffer = self.buffer[boundary + 2..].to_owned();
            if frame.is_empty() {
                continue;
            }
            let mut data = Vec::new();
            for line in frame.lines() {
                if let Some(value) = line.strip_prefix("data:") {
                    data.push(value.trim().to_owned());
                }
            }
            if !data.is_empty() {
                events.push(data.join("\n"));
            }
        }
        events
    }
}

#[cfg(test)]
mod tests;
