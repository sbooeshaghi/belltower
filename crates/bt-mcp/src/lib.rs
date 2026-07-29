#![forbid(unsafe_code)]

use bt_core::{
    ApprovalRequirement, BelltowerConfig, BelltowerError, McpServerConfig, McpServerDescriptor,
    McpServerStatus, McpToolDescriptor, McpTransportConfig, McpTransportKind, Result, ToolCallId,
    ToolContext, ToolDisplayGroup, ToolInterruptBehavior, ToolMetadata, ToolResultEnvelope,
    ToolRiskClass, ToolSpec,
    traits::{ToolExecutor, ToolFuture},
};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client as HttpClient, Url};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

const MCP_TOOL_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Clone)]
pub struct McpRegisteredTool {
    pub descriptor: McpToolDescriptor,
    pub executor: Arc<dyn ToolExecutor>,
}

#[derive(Clone, Debug, Default)]
pub struct McpInventorySnapshot {
    pub servers: Vec<McpServerDescriptor>,
    pub tools: Vec<McpToolDescriptor>,
}

#[derive(Clone, Default)]
pub struct McpRegistry {
    servers: BTreeMap<String, Arc<McpServerHandle>>,
}

impl McpRegistry {
    #[must_use]
    pub fn from_config(config: &BelltowerConfig) -> Self {
        let servers = config
            .mcp
            .servers
            .iter()
            .cloned()
            .map(|server| {
                let name = server.name.clone();
                (name, Arc::new(McpServerHandle::new(server)))
            })
            .collect();
        Self { servers }
    }

    pub async fn servers(&self) -> Vec<McpServerDescriptor> {
        let mut descriptors = Vec::with_capacity(self.servers.len());
        for server in self.servers.values() {
            descriptors.push(server.descriptor().await);
        }
        descriptors
    }

    pub async fn tools(&self) -> Result<Vec<McpToolDescriptor>> {
        let mut tools = Vec::new();
        for server in self.servers.values() {
            if let Ok(registered) = server.registered_tools().await {
                for tool in registered {
                    tools.push(tool.descriptor);
                }
            }
        }
        Ok(tools)
    }

    pub async fn inventory(&self) -> Result<McpInventorySnapshot> {
        let mut snapshot = McpInventorySnapshot::default();
        for server in self.servers.values() {
            let (descriptor, mut tools) = server.inventory().await;
            snapshot.servers.push(descriptor);
            snapshot.tools.append(&mut tools);
        }
        Ok(snapshot)
    }

    pub async fn registered_tools(&self) -> Result<Vec<McpRegisteredTool>> {
        let mut tools = Vec::new();
        for server in self.servers.values() {
            if let Ok(registered) = server.registered_tools().await {
                tools.extend(registered);
            }
        }
        Ok(tools)
    }

    pub async fn reload(&self) -> Result<()> {
        for server in self.servers.values() {
            server.reload().await?;
        }
        Ok(())
    }
}

struct McpServerHandle {
    config: McpServerConfig,
    session: Mutex<Option<StdioSession>>,
    http_session: Mutex<Option<HttpSession>>,
    cached_tools: Mutex<Option<Vec<DiscoveredTool>>>,
    last_error: Mutex<Option<String>>,
    tool_call_timeout: std::time::Duration,
}

impl McpServerHandle {
    fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            session: Mutex::new(None),
            http_session: Mutex::new(None),
            cached_tools: Mutex::new(None),
            last_error: Mutex::new(None),
            tool_call_timeout: MCP_TOOL_CALL_TIMEOUT,
        }
    }

    async fn descriptor(&self) -> McpServerDescriptor {
        self.base_descriptor(self.current_status().await)
    }

    async fn inventory(&self) -> (McpServerDescriptor, Vec<McpToolDescriptor>) {
        if !self.config.enabled {
            return (
                self.base_descriptor(McpServerStatus::Configured),
                Vec::new(),
            );
        }

        let status = self.current_status().await;
        if let Some(cached) = self.cached_tools.lock().await.clone() {
            return (
                self.base_descriptor(status),
                cached.into_iter().map(|tool| tool.descriptor).collect(),
            );
        }

        match self.discover_tools().await {
            Ok(discovered) => (
                self.base_descriptor(McpServerStatus::Ready),
                discovered.into_iter().map(|tool| tool.descriptor).collect(),
            ),
            Err(error) => (
                self.base_descriptor(McpServerStatus::Degraded {
                    reason: error.to_string(),
                }),
                Vec::new(),
            ),
        }
    }

    async fn current_status(&self) -> McpServerStatus {
        let last_error = self.last_error.lock().await.clone();
        let has_cached_tools = self.cached_tools.lock().await.is_some();
        let has_live_session = self.session.lock().await.is_some()
            || self
                .http_session
                .lock()
                .await
                .as_ref()
                .is_some_and(|session| session.initialized);
        if !self.config.enabled {
            McpServerStatus::Configured
        } else if let Some(error) = last_error {
            McpServerStatus::Degraded { reason: error }
        } else if has_cached_tools {
            McpServerStatus::Ready
        } else if has_live_session {
            McpServerStatus::Discovered
        } else {
            McpServerStatus::Configured
        }
    }

    fn base_descriptor(&self, status: McpServerStatus) -> McpServerDescriptor {
        let transport = match self.config.transport {
            McpTransportConfig::Stdio { .. } => McpTransportKind::Stdio,
            McpTransportConfig::StreamableHttp { .. } => McpTransportKind::StreamableHttp,
        };
        McpServerDescriptor {
            name: self.config.name.clone(),
            transport,
            enabled: self.config.enabled,
            status,
        }
    }

    async fn registered_tools(self: &Arc<Self>) -> Result<Vec<McpRegisteredTool>> {
        let discovered = self.discover_tools().await?;
        Ok(discovered
            .into_iter()
            .map(|tool| McpRegisteredTool {
                descriptor: tool.descriptor.clone(),
                executor: Arc::new(McpToolExecutor {
                    server: Arc::clone(self),
                    descriptor: tool.descriptor,
                    original_name: tool.original_name,
                }) as Arc<dyn ToolExecutor>,
            })
            .collect())
    }

    async fn discover_tools(&self) -> Result<Vec<DiscoveredTool>> {
        if !self.config.enabled {
            return Ok(Vec::new());
        }
        if let Some(cached) = self.cached_tools.lock().await.clone() {
            return Ok(cached);
        }

        let discovered = match &self.config.transport {
            McpTransportConfig::Stdio { .. } => {
                let mut session_guard = self.session.lock().await;
                let session = match ensure_stdio_session(&self.config, &mut session_guard).await {
                    Ok(session) => session,
                    Err(error) => {
                        self.set_error(error.to_string()).await;
                        return Err(error);
                    }
                };
                match session.list_tools().await {
                    Ok(tools) => tools
                        .into_iter()
                        .map(|tool| DiscoveredTool {
                            original_name: tool.name.clone(),
                            descriptor: McpToolDescriptor {
                                server_name: self.config.name.clone(),
                                tool_name: tool.name.clone(),
                                qualified_name: qualify_tool_name(&self.config.name, &tool.name),
                                description: format!(
                                    "[MCP:{}] {}",
                                    self.config.name,
                                    tool.description.unwrap_or_else(|| tool.name.clone())
                                ),
                                parameters_schema: tool.input_schema,
                            },
                        })
                        .collect::<Vec<_>>(),
                    Err(error) => {
                        *session_guard = None;
                        self.set_error(error.to_string()).await;
                        return Err(error);
                    }
                }
            }
            McpTransportConfig::StreamableHttp { .. } => {
                let mut session_guard = self.http_session.lock().await;
                let session = ensure_http_session(&self.config, &mut session_guard)?;
                match session.list_tools().await {
                    Ok(tools) => tools
                        .into_iter()
                        .map(|tool| DiscoveredTool {
                            original_name: tool.name.clone(),
                            descriptor: McpToolDescriptor {
                                server_name: self.config.name.clone(),
                                tool_name: tool.name.clone(),
                                qualified_name: qualify_tool_name(&self.config.name, &tool.name),
                                description: format!(
                                    "[MCP:{}] {}",
                                    self.config.name,
                                    tool.description.unwrap_or_else(|| tool.name.clone())
                                ),
                                parameters_schema: tool.input_schema,
                            },
                        })
                        .collect::<Vec<_>>(),
                    Err(error) => {
                        *session_guard = None;
                        self.set_error(error.to_string()).await;
                        return Err(error);
                    }
                }
            }
        };

        self.clear_error().await;
        *self.cached_tools.lock().await = Some(discovered.clone());
        Ok(discovered)
    }

    async fn call_tool(&self, original_name: &str, arguments: Value) -> Result<McpCallOutcome> {
        if !self.config.enabled {
            return Err(BelltowerError::InvalidState(format!(
                "MCP server `{}` is disabled",
                self.config.name
            )));
        }

        match &self.config.transport {
            McpTransportConfig::Stdio { .. } => {
                let mut session_guard = self.session.lock().await;
                let session = match ensure_stdio_session(&self.config, &mut session_guard).await {
                    Ok(session) => session,
                    Err(error) => {
                        self.set_error(error.to_string()).await;
                        return Err(error);
                    }
                };
                match tokio::time::timeout(
                    self.tool_call_timeout,
                    session.call_tool(original_name, arguments),
                )
                .await
                {
                    Ok(Ok(result)) => {
                        self.clear_error().await;
                        Ok(result)
                    }
                    Ok(Err(error)) => {
                        *session_guard = None;
                        self.set_error(error.to_string()).await;
                        Err(error)
                    }
                    Err(_) => {
                        *session_guard = None;
                        let error = BelltowerError::Protocol(format!(
                            "MCP server `{}` tool call timed out after {} seconds",
                            self.config.name,
                            self.tool_call_timeout.as_secs_f64()
                        ));
                        self.set_error(error.to_string()).await;
                        Err(error)
                    }
                }
            }
            McpTransportConfig::StreamableHttp { .. } => {
                let mut session_guard = self.http_session.lock().await;
                let session = match ensure_http_session(&self.config, &mut session_guard) {
                    Ok(session) => session,
                    Err(error) => {
                        self.set_error(error.to_string()).await;
                        return Err(error);
                    }
                };
                match tokio::time::timeout(
                    self.tool_call_timeout,
                    session.call_tool(original_name, arguments),
                )
                .await
                {
                    Ok(Ok(result)) => {
                        self.clear_error().await;
                        Ok(result)
                    }
                    Ok(Err(error)) => {
                        *session_guard = None;
                        self.set_error(error.to_string()).await;
                        Err(error)
                    }
                    Err(_) => {
                        *session_guard = None;
                        let error = BelltowerError::Protocol(format!(
                            "MCP server `{}` tool call timed out after {} seconds",
                            self.config.name,
                            self.tool_call_timeout.as_secs_f64()
                        ));
                        self.set_error(error.to_string()).await;
                        Err(error)
                    }
                }
            }
        }
    }

    async fn reload(&self) -> Result<()> {
        *self.cached_tools.lock().await = None;
        self.clear_error().await;
        if let Some(mut session) = self.session.lock().await.take() {
            session.shutdown().await?;
        }
        *self.http_session.lock().await = None;
        Ok(())
    }

    async fn set_error(&self, error: String) {
        *self.last_error.lock().await = Some(error);
    }

    async fn clear_error(&self) {
        *self.last_error.lock().await = None;
    }
}

#[derive(Clone, Debug)]
struct DiscoveredTool {
    original_name: String,
    descriptor: McpToolDescriptor,
}

#[derive(Clone, Debug, PartialEq)]
struct McpCallOutcome {
    is_error: bool,
    output: Value,
}

#[derive(Clone)]
struct McpToolExecutor {
    server: Arc<McpServerHandle>,
    descriptor: McpToolDescriptor,
    original_name: String,
}

impl ToolExecutor for McpToolExecutor {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.descriptor.qualified_name.clone(),
            description: self.descriptor.description.clone(),
            parameters_schema: self.descriptor.parameters_schema.clone(),
            metadata: ToolMetadata {
                risk_class: ToolRiskClass::High,
                is_read_only: false,
                is_concurrency_safe: false,
                interrupt_behavior: ToolInterruptBehavior::WaitForCompletion,
                execution_mode: bt_core::ToolExecutionMode::Immediate,
                should_defer: true,
                catalogue_tags: vec![
                    "mcp".to_owned(),
                    self.descriptor.server_name.clone(),
                    self.descriptor.tool_name.clone(),
                ],
                display_group: ToolDisplayGroup::External,
            },
        }
    }

    fn approval_requirement(&self, _arguments: &Value) -> ApprovalRequirement {
        ApprovalRequirement::Conditional {
            description: format!(
                "MCP tool `{}` on server `{}`",
                self.descriptor.tool_name, self.descriptor.server_name
            ),
        }
    }

    fn execute(&self, arguments: Value, _context: ToolContext) -> ToolFuture<'_> {
        let server = Arc::clone(&self.server);
        let descriptor = self.descriptor.clone();
        let original_name = self.original_name.clone();
        Box::pin(async move {
            let started = Instant::now();
            let (call_id, arguments) = split_call_id(arguments, &descriptor.qualified_name);
            let outcome = server.call_tool(&original_name, arguments).await?;
            Ok(ToolResultEnvelope {
                call_id,
                tool_name: descriptor.qualified_name,
                is_error: outcome.is_error,
                output: outcome.output,
                duration_ms: Some(started.elapsed().as_millis() as u64),
            })
        })
    }
}

fn split_call_id(arguments: Value, fallback_name: &str) -> (ToolCallId, Value) {
    match arguments {
        Value::Object(mut object) => {
            let call_id = object
                .remove("call_id")
                .and_then(|value| value.as_str().map(ToOwned::to_owned))
                .unwrap_or_else(|| format!("{fallback_name}_call"));
            (ToolCallId::new(call_id), Value::Object(object))
        }
        value => (ToolCallId::new(format!("{fallback_name}_call")), value),
    }
}

async fn ensure_stdio_session<'a>(
    config: &McpServerConfig,
    slot: &'a mut Option<StdioSession>,
) -> Result<&'a mut StdioSession> {
    if slot.is_none() {
        *slot = Some(StdioSession::spawn(config).await?);
    }
    slot.as_mut()
        .ok_or_else(|| BelltowerError::InvalidState("stdio MCP session is missing".to_owned()))
}

fn ensure_http_session<'a>(
    config: &McpServerConfig,
    slot: &'a mut Option<HttpSession>,
) -> Result<&'a mut HttpSession> {
    if slot.is_none() {
        *slot = Some(HttpSession::new(config)?);
    }
    slot.as_mut()
        .ok_or_else(|| BelltowerError::InvalidState("http MCP session is missing".to_owned()))
}

struct StdioSession {
    child: Child,
    stdout: BufReader<ChildStdout>,
    stdin: ChildStdin,
    next_id: u64,
}

struct HttpSession {
    client: HttpClient,
    base_url: Url,
    headers: HeaderMap,
    next_id: u64,
    initialized: bool,
}

impl HttpSession {
    fn new(config: &McpServerConfig) -> Result<Self> {
        let McpTransportConfig::StreamableHttp { base_url, headers } = &config.transport else {
            return Err(BelltowerError::Unsupported(
                "only streamable HTTP MCP transport can create an HTTP session".to_owned(),
            ));
        };

        Ok(Self {
            client: HttpClient::new(),
            base_url: base_url.clone(),
            headers: build_header_map(headers)?,
            next_id: 1,
            initialized: false,
        })
    }

    async fn list_tools(&mut self) -> Result<Vec<McpTool>> {
        self.ensure_initialized().await?;
        let response = self.request("tools/list", json!({})).await?;
        let tools = response
            .get("tools")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        serde_json::from_value(tools).map_err(BelltowerError::from)
    }

    async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<McpCallOutcome> {
        self.ensure_initialized().await?;
        let response = self
            .request(
                "tools/call",
                json!({
                    "name": name,
                    "arguments": arguments,
                }),
            )
            .await?;
        let is_error = response
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let output = response
            .get("structuredContent")
            .cloned()
            .unwrap_or(response);
        Ok(McpCallOutcome { is_error, output })
    }

    async fn ensure_initialized(&mut self) -> Result<()> {
        if self.initialized {
            return Ok(());
        }
        let _ = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "belltower",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }),
            )
            .await?;
        self.notify("notifications/initialized", json!({})).await?;
        self.initialized = true;
        Ok(())
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let response = self
            .client
            .post(self.base_url.clone())
            .headers(self.headers.clone())
            .json(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            }))
            .send()
            .await
            .map_err(|error| BelltowerError::Protocol(error.to_string()))?
            .error_for_status()
            .map_err(|error| BelltowerError::Protocol(error.to_string()))?;
        let value = decode_http_rpc_response(response).await?;
        if let Some(error) = value.get("error") {
            return Err(BelltowerError::Protocol(error.to_string()));
        }
        Ok(value.get("result").cloned().unwrap_or_else(|| json!({})))
    }

    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.client
            .post(self.base_url.clone())
            .headers(self.headers.clone())
            .json(&json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            }))
            .send()
            .await
            .map_err(|error| BelltowerError::Protocol(error.to_string()))?
            .error_for_status()
            .map_err(|error| BelltowerError::Protocol(error.to_string()))?;
        Ok(())
    }
}

async fn decode_http_rpc_response(response: reqwest::Response) -> Result<Value> {
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if content_type.starts_with("text/event-stream") {
        let text = response
            .text()
            .await
            .map_err(|error| BelltowerError::Protocol(error.to_string()))?;
        return decode_sse_rpc_response(&text);
    }
    response
        .json()
        .await
        .map_err(|error| BelltowerError::Protocol(error.to_string()))
}

fn decode_sse_rpc_response(text: &str) -> Result<Value> {
    let mut data = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim_end_matches('\r').strip_prefix("data:") else {
            continue;
        };
        let value = rest.strip_prefix(' ').unwrap_or(rest);
        if value.trim() == "[DONE]" {
            continue;
        }
        data.push(value.to_owned());
    }
    if data.is_empty() {
        return Err(BelltowerError::Protocol(
            "streamable HTTP MCP response did not contain SSE data".to_owned(),
        ));
    }
    serde_json::from_str(&data.join("\n")).map_err(BelltowerError::from)
}

impl StdioSession {
    async fn spawn(config: &McpServerConfig) -> Result<Self> {
        let McpTransportConfig::Stdio {
            command,
            args,
            cwd,
            env,
        } = &config.transport
        else {
            return Err(BelltowerError::Unsupported(
                "only stdio MCP transport can spawn a session".to_owned(),
            ));
        };

        let mut child = Command::new(command);
        child.args(args);
        child.stdin(std::process::Stdio::piped());
        child.stdout(std::process::Stdio::piped());
        child.stderr(std::process::Stdio::null());
        if let Some(cwd) = cwd {
            child.current_dir(cwd.as_std_path());
        }
        for (key, value) in env {
            child.env(key, value);
        }

        let mut child = child.spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| {
            BelltowerError::InvalidState("MCP stdio server missing stdout".to_owned())
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            BelltowerError::InvalidState("MCP stdio server missing stdin".to_owned())
        })?;

        let mut session = Self {
            child,
            stdout: BufReader::new(stdout),
            stdin,
            next_id: 1,
        };
        session.initialize().await?;
        Ok(session)
    }

    async fn initialize(&mut self) -> Result<()> {
        let _ = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "belltower",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }),
            )
            .await?;
        self.notify("notifications/initialized", json!({})).await
    }

    async fn list_tools(&mut self) -> Result<Vec<McpTool>> {
        let response = self.request("tools/list", json!({})).await?;
        let tools = response
            .get("tools")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        serde_json::from_value(tools).map_err(BelltowerError::from)
    }

    async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<McpCallOutcome> {
        let response = self
            .request(
                "tools/call",
                json!({
                    "name": name,
                    "arguments": arguments,
                }),
            )
            .await?;
        let is_error = response
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let output = response
            .get("structuredContent")
            .cloned()
            .unwrap_or(response);
        Ok(McpCallOutcome { is_error, output })
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;

        write_frame(
            &mut self.stdin,
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            }),
        )
        .await?;

        loop {
            let frame = read_frame(&mut self.stdout).await?;
            if frame.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = frame.get("error") {
                return Err(BelltowerError::Protocol(error.to_string()));
            }
            return Ok(frame.get("result").cloned().unwrap_or_else(|| json!({})));
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        write_frame(
            &mut self.stdin,
            &json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            }),
        )
        .await
    }

    async fn shutdown(&mut self) -> Result<()> {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
struct McpTool {
    name: String,
    description: Option<String>,
    #[serde(rename = "inputSchema", default = "default_schema")]
    input_schema: Value,
}

fn default_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": true
    })
}

async fn write_frame(writer: &mut ChildStdin, payload: &Value) -> Result<()> {
    let body = serde_json::to_vec(payload)?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_frame(reader: &mut BufReader<ChildStdout>) -> Result<Value> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).await?;
        if bytes == 0 {
            return Err(BelltowerError::Protocol(
                "MCP server closed the stdio stream".to_owned(),
            ));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            let length = value.trim().parse::<usize>().map_err(|error| {
                BelltowerError::Protocol(format!("invalid MCP content length: {error}"))
            })?;
            content_length = Some(length);
        }
    }

    let content_length = content_length.ok_or_else(|| {
        BelltowerError::Protocol("MCP frame is missing Content-Length".to_owned())
    })?;
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body)?)
}

fn qualify_tool_name(server_name: &str, tool_name: &str) -> String {
    let base = format!(
        "mcp_{}_{}",
        normalize_tool_component(server_name),
        normalize_tool_component(tool_name)
    );
    if base.len() <= 64 {
        return base;
    }

    let mut hasher = DefaultHasher::new();
    base.hash(&mut hasher);
    let digest = format!("{:016x}", hasher.finish());
    let prefix_len = 64usize.saturating_sub(digest.len() + 1);
    format!("{}_{}", &base[..prefix_len], digest)
}

fn normalize_tool_component(value: &str) -> String {
    let mut normalized = String::new();
    let mut last_was_underscore = false;
    for ch in value.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '_'
        };
        if mapped == '_' {
            if last_was_underscore {
                continue;
            }
            last_was_underscore = true;
        } else {
            last_was_underscore = false;
        }
        normalized.push(mapped);
    }
    let normalized = normalized.trim_matches('_').to_owned();
    if normalized.is_empty() {
        "tool".to_owned()
    } else {
        normalized
    }
}

fn build_header_map(headers: &BTreeMap<String, String>) -> Result<HeaderMap> {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        let name = HeaderName::try_from(name.as_str()).map_err(|error| {
            BelltowerError::Config(format!("invalid MCP header name `{name}`: {error}"))
        })?;
        let value = HeaderValue::from_str(value).map_err(|error| {
            BelltowerError::Config(format!("invalid MCP header value for `{name}`: {error}"))
        })?;
        map.insert(name, value);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::{McpCallOutcome, McpServerHandle, normalize_tool_component, qualify_tool_name};
    use axum::http::header;
    use axum::response::IntoResponse;
    use axum::{Json, Router, routing::post};
    use bt_core::{McpServerConfig, McpServerStatus, McpTransportConfig};
    use reqwest::Url;
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use tokio::net::TcpListener;

    #[test]
    fn qualified_names_are_provider_safe() {
        let qualified = qualify_tool_name("Filesystem Server", "read-file/path");
        assert!(qualified.starts_with("mcp_"));
        assert!(qualified.len() <= 64);
        assert_eq!(qualified, "mcp_filesystem_server_read_file_path");
    }

    #[test]
    fn normalization_collapses_punctuation() {
        assert_eq!(normalize_tool_component("Hello, world!"), "hello_world");
        assert_eq!(normalize_tool_component("___"), "tool");
    }

    fn http_server_config(base_url: Url) -> McpServerConfig {
        McpServerConfig {
            name: "fixture".to_owned(),
            enabled: true,
            transport: McpTransportConfig::StreamableHttp {
                base_url,
                headers: BTreeMap::new(),
            },
        }
    }

    async fn spawn_http_mcp_server() -> Url {
        async fn handle(Json(payload): Json<Value>) -> Json<Value> {
            let method = payload
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match method {
                "initialize" => Json(json!({
                    "jsonrpc": "2.0",
                    "id": payload.get("id").cloned().unwrap_or(Value::Null),
                    "result": {
                        "serverInfo": { "name": "fixture", "version": "0.1.0" },
                        "capabilities": {}
                    }
                })),
                "tools/list" => Json(json!({
                    "jsonrpc": "2.0",
                    "id": payload.get("id").cloned().unwrap_or(Value::Null),
                    "result": {
                        "tools": [{
                            "name": "search_docs",
                            "description": "Search docs",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "query": { "type": "string" }
                                }
                            }
                        }]
                    }
                })),
                "tools/call" => Json(json!({
                    "jsonrpc": "2.0",
                    "id": payload.get("id").cloned().unwrap_or(Value::Null),
                    "result": {
                        "structuredContent": { "ok": true }
                    }
                })),
                "notifications/initialized" => Json(json!({})),
                other => Json(json!({
                    "jsonrpc": "2.0",
                    "id": payload.get("id").cloned().unwrap_or(Value::Null),
                    "error": { "message": format!("unexpected method {other}") }
                })),
            }
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/", post(handle)))
                .await
                .expect("serve");
        });
        Url::parse(&format!("http://{addr}/")).expect("url")
    }

    async fn spawn_sse_http_mcp_server() -> Url {
        async fn handle(Json(payload): Json<Value>) -> axum::response::Response {
            let method = payload
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let id = payload.get("id").cloned().unwrap_or(Value::Null);
            let body = match method {
                "initialize" => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "serverInfo": { "name": "fixture", "version": "0.1.0" },
                        "capabilities": {}
                    }
                }),
                "tools/list" => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "tools": [{
                            "name": "search_docs",
                            "description": "Search docs",
                            "inputSchema": { "type": "object" }
                        }]
                    }
                }),
                "notifications/initialized" => json!({}),
                other => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "message": format!("unexpected method {other}") }
                }),
            };
            (
                [(header::CONTENT_TYPE, "text/event-stream")],
                format!("event: message\ndata: {body}\n\n"),
            )
                .into_response()
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/", post(handle)))
                .await
                .expect("serve");
        });
        Url::parse(&format!("http://{addr}/")).expect("url")
    }

    async fn spawn_stalled_call_http_mcp_server() -> Url {
        async fn handle(Json(payload): Json<Value>) -> Json<Value> {
            let method = payload
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if method == "tools/call" {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            let id = payload.get("id").cloned().unwrap_or(Value::Null);
            let result = match method {
                "initialize" => json!({
                    "serverInfo": { "name": "fixture", "version": "0.1.0" },
                    "capabilities": {}
                }),
                "tools/call" => json!({"structuredContent": {"late": true}}),
                _ => json!({}),
            };
            Json(json!({"jsonrpc": "2.0", "id": id, "result": result}))
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/", post(handle)))
                .await
                .expect("serve");
        });
        Url::parse(&format!("http://{addr}/")).expect("url")
    }

    #[tokio::test]
    async fn descriptor_reports_discovered_after_direct_tool_call_without_cached_inventory() {
        let base_url = spawn_http_mcp_server().await;
        let handle = McpServerHandle::new(http_server_config(base_url));

        let outcome = handle
            .call_tool("search_docs", json!({ "query": "mcp" }))
            .await
            .expect("call tool");
        assert_eq!(
            outcome,
            McpCallOutcome {
                is_error: false,
                output: json!({ "ok": true }),
            }
        );

        let descriptor = handle.descriptor().await;
        assert!(matches!(descriptor.status, McpServerStatus::Discovered));
    }

    #[tokio::test]
    async fn descriptor_reports_ready_after_tool_discovery_caches_inventory() {
        let base_url = spawn_http_mcp_server().await;
        let handle = std::sync::Arc::new(McpServerHandle::new(http_server_config(base_url)));

        let registered = handle.registered_tools().await.expect("registered tools");
        assert_eq!(registered.len(), 1);

        let descriptor = handle.descriptor().await;
        assert!(matches!(descriptor.status, McpServerStatus::Ready));
    }

    #[tokio::test]
    async fn streamable_http_decodes_sse_rpc_responses() {
        let base_url = spawn_sse_http_mcp_server().await;
        let handle = std::sync::Arc::new(McpServerHandle::new(http_server_config(base_url)));

        let registered = handle.registered_tools().await.expect("registered tools");

        assert_eq!(registered.len(), 1);
        assert_eq!(
            registered[0].descriptor.qualified_name,
            "mcp_fixture_search_docs"
        );
    }

    #[tokio::test]
    async fn inventory_returns_servers_and_tools_from_same_snapshot() {
        let base_url = spawn_http_mcp_server().await;
        let mut servers = BTreeMap::new();
        servers.insert(
            "fixture".to_owned(),
            std::sync::Arc::new(McpServerHandle::new(http_server_config(base_url))),
        );
        let registry = super::McpRegistry { servers };

        let inventory = registry.inventory().await.expect("inventory");
        assert_eq!(inventory.servers.len(), 1);
        assert_eq!(inventory.tools.len(), 1);
        assert!(matches!(
            inventory.servers[0].status,
            McpServerStatus::Ready
        ));
        assert_eq!(inventory.tools[0].qualified_name, "mcp_fixture_search_docs");
    }

    #[tokio::test]
    async fn inventory_keeps_degraded_status_even_when_cached_tools_exist() {
        let base_url = spawn_http_mcp_server().await;
        let handle = McpServerHandle::new(http_server_config(base_url));

        let discovered = handle.discover_tools().await.expect("discover tools");
        assert_eq!(discovered.len(), 1);
        handle.set_error("call failed".to_owned()).await;

        let (descriptor, tools) = handle.inventory().await;
        assert!(matches!(
            descriptor.status,
            McpServerStatus::Degraded { ref reason } if reason == "call failed"
        ));
        assert_eq!(tools.len(), 1);
    }

    #[tokio::test]
    async fn stalled_tool_call_is_bounded_resets_session_and_marks_server_degraded() {
        let base_url = spawn_stalled_call_http_mcp_server().await;
        let mut handle = McpServerHandle::new(http_server_config(base_url));
        handle.tool_call_timeout = std::time::Duration::from_millis(30);

        let error = handle
            .call_tool("search_docs", json!({"query": "hold"}))
            .await
            .expect_err("stalled call must time out");
        assert!(error.to_string().contains("timed out"));
        assert!(handle.http_session.lock().await.is_none());
        let descriptor = handle.descriptor().await;
        assert!(matches!(
            descriptor.status,
            McpServerStatus::Degraded { ref reason } if reason.contains("timed out")
        ));
    }
}
