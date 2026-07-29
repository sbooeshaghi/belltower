use crate::text::{MAX_TOOL_RESULT_BYTES, MAX_TOOL_RESULT_LINES, truncate_text};
use bt_core::{
    ApprovalRequirement, BelltowerConfig, BelltowerError, Result, ToolCallId, ToolContext,
    ToolDisplayGroup, ToolExecutionMode, ToolExecutor, ToolFuture, ToolInterruptBehavior,
    ToolMetadata, ToolResultEnvelope, ToolRiskClass, ToolSpec, WebBackendConfig, WebBackendKind,
    WebFetchFormat, WebFetchResponse, WebRetrievalConfig, WebSearchRequest, WebSearchResponse,
    WebSearchResult,
};
use reqwest::header::{CONTENT_TYPE, LOCATION};
use reqwest::{Client, Response, Url};
use serde_json::{Value, json};
use std::env;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::lookup_host;

type BackendFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;
type ResolverFuture<'a> =
    Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + 'a>>;

const WEB_FETCH_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const WEB_FETCH_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);

trait WebSearchBackend: Send + Sync {
    fn name(&self) -> &str;
    fn search<'a>(&'a self, request: WebSearchRequest) -> BackendFuture<'a, WebSearchResponse>;
}

trait WebFetchBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn fetch<'a>(
        &'a self,
        url: &'a str,
        format: WebFetchFormat,
        max_bytes: usize,
    ) -> BackendFuture<'a, WebFetchResponse>;
}

#[derive(Clone)]
pub struct WebSearchTool {
    config: WebRetrievalConfig,
    credentials: Vec<WebSearchCredential>,
}

/// Resolved in-memory credential data for a configured web-search backend.
///
/// Credential resolution belongs to the server/launcher/auth seam. The tool
/// crate receives only this execution-time snapshot so `bt-agent` can depend on
/// built-in tools without inheriting auth-store behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebSearchCredential {
    pub backend_id: String,
    pub api_key: String,
}

impl WebSearchTool {
    #[must_use]
    pub fn new(config: WebRetrievalConfig) -> Self {
        Self::new_with_credentials(config, Vec::new())
    }

    #[must_use]
    pub fn new_with_credentials(
        config: WebRetrievalConfig,
        credentials: Vec<WebSearchCredential>,
    ) -> Self {
        Self {
            config,
            credentials,
        }
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new(
            BelltowerConfig::from_embedded()
                .expect("embedded Belltower config should be valid")
                .web,
        )
    }
}

impl ToolExecutor for WebSearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "web_search".to_owned(),
            description: "Search the public web and return ranked, source-attributed results."
                .to_owned(),
            parameters_schema: json!({
                "type": "object",
                "required": ["query", "call_id"],
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Natural-language or keyword search query."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 10,
                        "description": "Maximum number of results to return. Defaults to 5."
                    },
                    "site": {
                        "type": "string",
                        "description": "Optional domain constraint such as example.org."
                    },
                    "backend": {
                        "type": "string",
                        "description": "Optional backend override. Currently supports exa."
                    },
                    "call_id": {"type": "string"}
                }
            }),
            metadata: web_metadata(vec![
                "web".to_owned(),
                "search".to_owned(),
                "external".to_owned(),
            ]),
        }
    }

    fn approval_requirement(&self, arguments: &Value) -> ApprovalRequirement {
        let description = arguments
            .get("query")
            .and_then(Value::as_str)
            .map(|query| format!("Search the public web for `{query}`"))
            .unwrap_or_else(|| "Search the public web".to_owned());
        ApprovalRequirement::Conditional { description }
    }

    fn execute(&self, arguments: Value, _context: ToolContext) -> ToolFuture<'_> {
        let config = self.config.clone();
        let credentials = self.credentials.clone();
        Box::pin(async move {
            let call_id = required_string(&arguments, "call_id")?;
            let query = required_string(&arguments, "query")?;
            let max_results = arguments
                .get("max_results")
                .and_then(Value::as_u64)
                .map(|value| value.clamp(1, 10) as u8);
            let site = arguments
                .get("site")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            let backend = arguments
                .get("backend")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);

            let request = WebSearchRequest {
                query,
                max_results,
                site,
                backend: backend.clone(),
            };
            let backend = select_search_backend(backend.as_deref(), &config, &credentials)?;
            let response = backend.search(request).await?;
            Ok(ToolResultEnvelope {
                call_id: ToolCallId::new(call_id),
                tool_name: "web_search".to_owned(),
                is_error: false,
                output: serde_json::to_value(response).map_err(json_error)?,
                duration_ms: None,
            })
        })
    }
}

pub struct WebFetchTool;

impl ToolExecutor for WebFetchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "web_fetch".to_owned(),
            description: "Fetch a public URL and return structured status, provenance, and text."
                .to_owned(),
            parameters_schema: json!({
                "type": "object",
                "required": ["url", "call_id"],
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "HTTP or HTTPS URL to fetch."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["text", "markdown", "raw"],
                        "description": "Response normalization. Defaults to text."
                    },
                    "max_bytes": {
                        "type": "integer",
                        "minimum": 1024,
                        "maximum": MAX_TOOL_RESULT_BYTES,
                        "description": "Maximum response bytes to keep in the tool result."
                    },
                    "call_id": {"type": "string"}
                }
            }),
            metadata: web_metadata(vec![
                "http".to_owned(),
                "web".to_owned(),
                "url".to_owned(),
                "fetch".to_owned(),
            ]),
        }
    }

    fn approval_requirement(&self, arguments: &Value) -> ApprovalRequirement {
        let description = arguments
            .get("url")
            .and_then(Value::as_str)
            .map(|url| format!("Fetch public URL `{url}`"))
            .unwrap_or_else(|| "Fetch a public URL".to_owned());
        ApprovalRequirement::Conditional { description }
    }

    fn execute(&self, arguments: Value, _context: ToolContext) -> ToolFuture<'_> {
        Box::pin(async move {
            let call_id = required_string(&arguments, "call_id")?;
            let url = required_string(&arguments, "url")?;
            let format = parse_fetch_format(arguments.get("format"))?;
            let max_bytes = arguments
                .get("max_bytes")
                .and_then(Value::as_u64)
                .map(|value| (value as usize).clamp(1024, MAX_TOOL_RESULT_BYTES))
                .unwrap_or(MAX_TOOL_RESULT_BYTES);
            let backend = DirectHttpFetchBackend::new();
            let response = backend.fetch(&url, format, max_bytes).await?;
            let is_error = !(200..400).contains(&response.status);

            Ok(ToolResultEnvelope {
                call_id: ToolCallId::new(call_id),
                tool_name: "web_fetch".to_owned(),
                is_error,
                output: serde_json::to_value(response).map_err(json_error)?,
                duration_ms: None,
            })
        })
    }
}

fn web_metadata(catalogue_tags: Vec<String>) -> ToolMetadata {
    ToolMetadata {
        risk_class: ToolRiskClass::Moderate,
        is_read_only: true,
        is_concurrency_safe: true,
        interrupt_behavior: ToolInterruptBehavior::Immediate,
        execution_mode: ToolExecutionMode::Immediate,
        should_defer: false,
        catalogue_tags,
        display_group: ToolDisplayGroup::Web,
    }
}

fn required_string(arguments: &Value, key: &str) -> Result<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| BelltowerError::Tool(format!("missing string argument `{key}`")))
}

fn parse_fetch_format(value: Option<&Value>) -> Result<WebFetchFormat> {
    match value.and_then(Value::as_str).unwrap_or("text") {
        "text" => Ok(WebFetchFormat::Text),
        "markdown" => Ok(WebFetchFormat::Markdown),
        "raw" => Ok(WebFetchFormat::Raw),
        other => Err(BelltowerError::Tool(format!(
            "unsupported web_fetch format `{other}`"
        ))),
    }
}

fn select_search_backend(
    requested: Option<&str>,
    config: &WebRetrievalConfig,
    credentials: &[WebSearchCredential],
) -> Result<Box<dyn WebSearchBackend>> {
    let requested = requested
        .map(str::to_ascii_lowercase)
        .or_else(|| {
            env::var("BELLTOWER_WEB_SEARCH_BACKEND")
                .ok()
                .map(|value| value.to_ascii_lowercase())
        })
        .or_else(|| {
            config
                .search_backend
                .as_ref()
                .map(|value| value.to_ascii_lowercase())
        });

    if let Some(backend_id) = requested.as_deref() {
        return configured_search_backend(config, credentials, backend_id);
    }

    let mut first_error = None;
    for backend in config.backends.iter().filter(|backend| backend.enabled) {
        match search_backend_from_config(backend, credentials) {
            Ok(backend) => return Ok(backend),
            Err(error) if first_error.is_none() => first_error = Some(error),
            Err(_) => {}
        }
    }
    Err(first_error.unwrap_or_else(|| {
        BelltowerError::Tool("no enabled web_search backend is configured".to_owned())
    }))
}

fn configured_search_backend(
    config: &WebRetrievalConfig,
    credentials: &[WebSearchCredential],
    backend_id: &str,
) -> Result<Box<dyn WebSearchBackend>> {
    let backend = config
        .backends
        .iter()
        .find(|backend| backend.id.eq_ignore_ascii_case(backend_id))
        .ok_or_else(|| {
            BelltowerError::Tool(format!("unsupported web_search backend `{backend_id}`"))
        })?;
    if !backend.enabled {
        return Err(BelltowerError::Tool(format!(
            "web_search backend `{}` is disabled in config",
            backend.id
        )));
    }
    search_backend_from_config(backend, credentials)
}

fn search_backend_from_config(
    backend: &WebBackendConfig,
    credentials: &[WebSearchCredential],
) -> Result<Box<dyn WebSearchBackend>> {
    match backend.kind {
        WebBackendKind::Exa => Ok(Box::new(ExaSearchBackend::from_config(
            backend,
            credentials,
        )?)),
    }
}

struct ExaSearchBackend {
    backend_id: String,
    base_url: Url,
    api_key: String,
}

impl ExaSearchBackend {
    fn from_config(config: &WebBackendConfig, credentials: &[WebSearchCredential]) -> Result<Self> {
        let credential = credentials
            .iter()
            .find(|credential| credential.backend_id.eq_ignore_ascii_case(&config.id))
            .ok_or_else(|| missing_web_search_auth_error(config))?;
        Ok(Self {
            backend_id: config.id.clone(),
            base_url: config.base_url.clone(),
            api_key: credential.api_key.clone(),
        })
    }
}

fn missing_web_search_auth_error(config: &WebBackendConfig) -> BelltowerError {
    BelltowerError::Tool(format!(
        "web_search backend `{}` is not configured; run `belltower web login {}` or set {}",
        config.id,
        config.id,
        if config.auth_sources.is_empty() {
            "a configured auth source".to_owned()
        } else {
            config.auth_sources.join(", ")
        }
    ))
}

impl WebSearchBackend for ExaSearchBackend {
    fn name(&self) -> &str {
        &self.backend_id
    }

    fn search<'a>(&'a self, request: WebSearchRequest) -> BackendFuture<'a, WebSearchResponse> {
        Box::pin(async move {
            let max_results = usize::from(request.max_results.unwrap_or(5).clamp(1, 10));
            let mut payload = json!({
                "query": request.query,
                "numResults": max_results,
            });
            if let Some(site) = request.site.as_deref().map(normalize_domain_constraint) {
                payload["includeDomains"] = json!([site]);
            }

            let search_url = self
                .base_url
                .join("search")
                .map_err(|error| BelltowerError::Tool(error.to_string()))?;
            let response = Client::new()
                .post(search_url)
                .header("x-api-key", &self.api_key)
                .json(&payload)
                .send()
                .await
                .map_err(reqwest_error)?;
            let status = response.status();
            let body = response.text().await.map_err(reqwest_error)?;
            if !status.is_success() {
                return Err(BelltowerError::Tool(format!(
                    "exa web_search failed with status {status}: {}",
                    truncate_text(&body, 20, 4_096)
                )));
            }
            let value = serde_json::from_str::<Value>(&body).map_err(json_error)?;
            Ok(normalize_exa_response(
                self.name(),
                payload["query"].as_str().unwrap_or_default(),
                max_results,
                &value,
            ))
        })
    }
}

fn normalize_domain_constraint(site: &str) -> String {
    site.trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_matches('/')
        .to_owned()
}

fn normalize_exa_response(
    backend: &str,
    query: &str,
    max_results: usize,
    payload: &Value,
) -> WebSearchResponse {
    let results = payload
        .get("results")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .take(max_results)
                .filter_map(normalize_exa_result)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    WebSearchResponse {
        backend: backend.to_owned(),
        query: query.to_owned(),
        result_count: results.len(),
        results,
    }
}

fn normalize_exa_result(value: &Value) -> Option<WebSearchResult> {
    let url = string_field(value, &["url"])?;
    Some(WebSearchResult {
        title: string_field(value, &["title"]),
        url,
        snippet: string_field(value, &["text", "summary", "snippet"]),
        source: "exa".to_owned(),
        published_at: string_field(value, &["publishedDate", "published_at"]),
        score: value.get("score").and_then(Value::as_f64),
    })
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

trait HostResolver: Send + Sync {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> ResolverFuture<'a>;
}

struct SystemHostResolver;

impl HostResolver for SystemHostResolver {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> ResolverFuture<'a> {
        Box::pin(async move { Ok(lookup_host((host, port)).await?.collect()) })
    }
}

struct DirectHttpFetchBackend {
    resolver: Arc<dyn HostResolver>,
    connect_timeout: Duration,
    total_timeout: Duration,
}

impl DirectHttpFetchBackend {
    fn new() -> Self {
        Self {
            resolver: Arc::new(SystemHostResolver),
            connect_timeout: WEB_FETCH_CONNECT_TIMEOUT,
            total_timeout: WEB_FETCH_TOTAL_TIMEOUT,
        }
    }
}

impl WebFetchBackend for DirectHttpFetchBackend {
    fn name(&self) -> &'static str {
        "direct_http"
    }

    fn fetch<'a>(
        &'a self,
        url: &'a str,
        format: WebFetchFormat,
        max_bytes: usize,
    ) -> BackendFuture<'a, WebFetchResponse> {
        Box::pin(async move {
            let requested = parse_public_http_url(url)?;
            let (final_url, status, content_type, bytes, truncated_by_bytes) =
                with_fetch_timeout(self.total_timeout, async {
                    let mut response = send_with_safe_redirects(
                        self.resolver.as_ref(),
                        requested.clone(),
                        self.connect_timeout,
                    )
                    .await?;
                    let final_url = response.url().to_string();
                    let status = response.status().as_u16();
                    let content_type = response
                        .headers()
                        .get(CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .map(ToOwned::to_owned);
                    let (bytes, truncated_by_bytes) =
                        read_response_body_limited(&mut response, max_bytes).await?;
                    Ok((final_url, status, content_type, bytes, truncated_by_bytes))
                })
                .await?;
            let raw_text = String::from_utf8_lossy(&bytes).into_owned();
            let title = content_type
                .as_deref()
                .filter(|content_type| content_type.contains("html"))
                .and_then(|_| extract_html_title(&raw_text));
            let normalized = match format {
                WebFetchFormat::Text | WebFetchFormat::Raw => raw_text,
                WebFetchFormat::Markdown => html_to_text(&raw_text),
            };
            let body = truncate_text(&normalized, MAX_TOOL_RESULT_LINES, max_bytes);
            let truncated = truncated_by_bytes || body != normalized;

            Ok(WebFetchResponse {
                backend: self.name().to_owned(),
                requested_url: requested.to_string(),
                final_url,
                status,
                content_type,
                title,
                format,
                body,
                bytes: bytes.len() as u64,
                truncated,
                content_digest: stable_content_digest(&bytes),
            })
        })
    }
}

async fn with_fetch_timeout<T>(
    total_timeout: Duration,
    fetch: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::time::timeout(total_timeout, fetch)
        .await
        .map_err(|_| {
            BelltowerError::Tool(format!(
                "web_fetch timed out after {} milliseconds",
                total_timeout.as_millis()
            ))
        })?
}

async fn read_response_body_limited(
    response: &mut Response,
    max_bytes: usize,
) -> Result<(Vec<u8>, bool)> {
    let cap = max_bytes.max(1);
    let mut bytes = Vec::with_capacity(cap.min(8192));
    let mut truncated = false;
    while let Some(chunk) = response.chunk().await.map_err(reqwest_error)? {
        if append_limited_bytes(&mut bytes, &chunk, cap) {
            truncated = true;
            break;
        }
    }
    Ok((bytes, truncated))
}

fn append_limited_bytes(bytes: &mut Vec<u8>, chunk: &[u8], cap: usize) -> bool {
    let remaining = cap.saturating_sub(bytes.len());
    if remaining == 0 {
        return true;
    }
    if chunk.len() > remaining {
        bytes.extend_from_slice(&chunk[..remaining]);
        return true;
    }
    bytes.extend_from_slice(chunk);
    false
}

fn parse_public_http_url(value: &str) -> Result<Url> {
    let url = Url::parse(value)
        .map_err(|error| BelltowerError::Tool(format!("invalid URL `{value}`: {error}")))?;
    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(BelltowerError::Tool(format!(
                "unsupported web_fetch URL scheme `{scheme}`"
            )));
        }
    }
    validate_url_host_without_dns(&url)?;
    Ok(url)
}

async fn send_with_safe_redirects(
    resolver: &dyn HostResolver,
    initial_url: Url,
    connect_timeout: Duration,
) -> Result<Response> {
    let mut current = initial_url;
    for _ in 0..=5 {
        let resolved = resolve_public_target(resolver, &current).await?;
        let client = pinned_client(&resolved, connect_timeout)?;
        let response = client
            .get(current.clone())
            .send()
            .await
            .map_err(reqwest_error)?;
        if !response.status().is_redirection() {
            return Ok(response);
        }
        let Some(location) = response.headers().get(LOCATION) else {
            return Ok(response);
        };
        let location = location
            .to_str()
            .map_err(|error| BelltowerError::Tool(format!("invalid redirect location: {error}")))?;
        current = validated_redirect_url(&current, location)?;
    }

    Err(BelltowerError::Tool(
        "too many redirects while fetching public URL".to_owned(),
    ))
}

struct ResolvedTarget {
    host: String,
    addrs: Vec<SocketAddr>,
    override_dns: bool,
}

async fn resolve_public_target(resolver: &dyn HostResolver, url: &Url) -> Result<ResolvedTarget> {
    validate_url_host_without_dns(url)?;
    let host = url
        .host_str()
        .ok_or_else(|| BelltowerError::Tool("URL must include a host".to_owned()))?;
    let port = url.port_or_known_default().unwrap_or(443);
    if let Some(ip) = parse_ip_literal(host) {
        return Ok(ResolvedTarget {
            host: host.to_owned(),
            addrs: vec![SocketAddr::new(ip, port)],
            override_dns: false,
        });
    }

    let mut addrs = resolver.resolve(host, port).await.map_err(|error| {
        BelltowerError::Tool(format!(
            "failed to resolve web_fetch host `{host}`: {error}"
        ))
    })?;
    if addrs.is_empty() {
        return Err(BelltowerError::Tool(format!(
            "web_fetch host `{host}` resolved to no addresses"
        )));
    }
    for addr in &addrs {
        if is_blocked_ip(addr.ip()) {
            return Err(BelltowerError::Tool(format!(
                "web_fetch blocked non-public resolved address `{}` for host `{host}`",
                addr.ip()
            )));
        }
    }
    addrs.sort_unstable();
    addrs.dedup();
    Ok(ResolvedTarget {
        host: host.to_owned(),
        addrs,
        override_dns: true,
    })
}

fn pinned_client(target: &ResolvedTarget, connect_timeout: Duration) -> Result<Client> {
    let mut builder = Client::builder()
        // Proxies would move DNS resolution back outside this trust boundary.
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(connect_timeout);
    if target.override_dns {
        builder = builder.resolve_to_addrs(&target.host, &target.addrs);
    }
    builder.build().map_err(reqwest_error)
}

fn validated_redirect_url(current: &Url, location: &str) -> Result<Url> {
    let redirected = current.join(location).map_err(|error| {
        BelltowerError::Tool(format!("invalid redirect location `{location}`: {error}"))
    })?;
    validate_url_host_without_dns(&redirected)?;
    Ok(redirected)
}

fn validate_url_host_without_dns(url: &Url) -> Result<()> {
    let host = url
        .host_str()
        .ok_or_else(|| BelltowerError::Tool("URL must include a host".to_owned()))?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err(BelltowerError::Tool(
            "web_fetch does not allow localhost URLs".to_owned(),
        ));
    }
    if let Some(ip) = parse_ip_literal(host)
        && is_blocked_ip(ip)
    {
        return Err(BelltowerError::Tool(format!(
            "web_fetch blocked non-public address `{ip}`"
        )));
    }
    Ok(())
}

fn parse_ip_literal(host: &str) -> Option<IpAddr> {
    host.parse::<IpAddr>().ok().or_else(|| {
        host.strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .and_then(|host| host.parse::<IpAddr>().ok())
    })
}

fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => !is_globally_routable_ipv4(ip),
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return is_blocked_ip(IpAddr::V4(mapped));
            }
            !is_globally_routable_ipv6(ip)
        }
    }
}

fn is_globally_routable_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, d] = ip.octets();
    !(a == 0
        || ip.is_private()
        || (a == 100 && (b & 0b1100_0000) == 0b0100_0000)
        || ip.is_loopback()
        || ip.is_link_local()
        || (a == 192 && b == 0 && c == 0 && d != 9 && d != 10)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 88 && c == 99)
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || (a == 198 && (b == 18 || b == 19))
        || (224..=239).contains(&a)
        || a >= 240)
}

fn is_globally_routable_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    let value = u128::from_be_bytes(ip.octets());
    let octets = ip.octets();
    let ietf_protocol_assignment = segments[0] == 0x2001 && segments[1] < 0x0200;
    let globally_routable_ietf_exception = value == 0x2001_0001_0000_0000_0000_0000_0000_0001
        || value == 0x2001_0001_0000_0000_0000_0000_0000_0002
        || value == 0x2001_0001_0000_0000_0000_0000_0000_0003
        || (segments[0] == 0x2001 && segments[1] == 0x0003)
        || (segments[0] == 0x2001 && segments[1] == 0x0004 && segments[2] == 0x0112)
        || (segments[0] == 0x2001 && (0x0020..=0x003f).contains(&segments[1]));
    let documentation =
        (segments[0] == 0x2001 && segments[1] == 0x0db8) || (segments[0] & 0xfff0) == 0x3ff0;
    let deprecated_site_local = (segments[0] & 0xffc0) == 0xfec0;
    let nat64_embeds_non_public_ipv4 = segments[..6] == [0x0064, 0xff9b, 0, 0, 0, 0]
        && !is_globally_routable_ipv4(Ipv4Addr::new(
            octets[12], octets[13], octets[14], octets[15],
        ));
    let ipv6_dummy_prefix = segments[..4] == [0x0100, 0, 0, 1];

    !(ip.is_unspecified()
        || ip.is_loopback()
        || (segments[..6] == [0, 0, 0, 0, 0, 0xffff])
        || (segments[0] == 0x0064 && segments[1] == 0xff9b && segments[2] == 1)
        || (segments[..4] == [0x0100, 0, 0, 0])
        || ipv6_dummy_prefix
        || nat64_embeds_non_public_ipv4
        || (ietf_protocol_assignment && !globally_routable_ietf_exception)
        || segments[0] == 0x2002
        || documentation
        || segments[0] == 0x5f00
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || deprecated_site_local
        || ip.is_multicast())
}

fn extract_html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start_tag = lower.find("<title")?;
    let content_start = lower[start_tag..].find('>')? + start_tag + 1;
    let content_end = lower[content_start..].find("</title>")? + content_start;
    Some(collapse_whitespace(&decode_basic_html_entities(
        &html[content_start..content_end],
    )))
    .filter(|title| !title.is_empty())
}

fn html_to_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                text.push(' ');
            }
            '>' => in_tag = false,
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }
    collapse_whitespace(&decode_basic_html_entities(&text))
}

fn decode_basic_html_entities(value: &str) -> String {
    value
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn stable_content_digest(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

fn reqwest_error(error: reqwest::Error) -> BelltowerError {
    BelltowerError::Tool(error.to_string())
}

fn json_error(error: serde_json::Error) -> BelltowerError {
    BelltowerError::Tool(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        DirectHttpFetchBackend, HostResolver, ResolvedTarget, ResolverFuture, WebFetchBackend,
        WebFetchTool, WebSearchTool, append_limited_bytes, extract_html_title, is_blocked_ip,
        normalize_exa_response, parse_public_http_url, pinned_client, read_response_body_limited,
        reqwest_error, resolve_public_target, send_with_safe_redirects, stable_content_digest,
        validated_redirect_url, with_fetch_timeout,
    };
    use bt_core::{ApprovalRequirement, ToolRiskClass, traits::ToolExecutor};
    use serde_json::json;
    use std::collections::VecDeque;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::time::timeout;

    struct ScriptedResolver {
        answers: Mutex<VecDeque<Vec<SocketAddr>>>,
        calls: AtomicUsize,
    }

    impl ScriptedResolver {
        fn new(answers: impl IntoIterator<Item = Vec<SocketAddr>>) -> Self {
            Self {
                answers: Mutex::new(answers.into_iter().collect()),
                calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl HostResolver for ScriptedResolver {
        fn resolve<'a>(&'a self, _host: &'a str, _port: u16) -> ResolverFuture<'a> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let answer = self
                .answers
                .lock()
                .expect("resolver answers lock")
                .pop_front()
                .unwrap_or_default();
            Box::pin(async move { Ok(answer) })
        }
    }

    struct HeldResolver;

    impl HostResolver for HeldResolver {
        fn resolve<'a>(&'a self, _host: &'a str, _port: u16) -> ResolverFuture<'a> {
            Box::pin(std::future::pending())
        }
    }

    fn public_addr(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)), port)
    }

    #[test]
    fn web_tools_are_moderate_risk_and_require_conditional_approval() {
        let search = WebSearchTool::default();
        let search_spec = search.spec();
        assert_eq!(search_spec.name, "web_search");
        assert_eq!(search_spec.metadata.risk_class, ToolRiskClass::Moderate);
        let requirement = search.approval_requirement(&json!({
            "query": "belltower agent harness",
            "call_id": "call-search"
        }));
        match requirement {
            ApprovalRequirement::Conditional { description } => {
                assert!(description.contains("belltower agent harness"));
            }
            other => panic!("expected conditional approval requirement, got {other:?}"),
        }

        let fetch = WebFetchTool;
        let fetch_spec = fetch.spec();
        assert_eq!(fetch_spec.name, "web_fetch");
        assert_eq!(fetch_spec.metadata.risk_class, ToolRiskClass::Moderate);
        let requirement = fetch.approval_requirement(&json!({
            "url": "https://example.com",
            "call_id": "call-fetch"
        }));
        match requirement {
            ApprovalRequirement::Conditional { description } => {
                assert!(description.contains("https://example.com"));
            }
            other => panic!("expected conditional approval requirement, got {other:?}"),
        }
    }

    #[test]
    fn web_fetch_rejects_local_and_private_urls_before_network() {
        assert!(parse_public_http_url("http://localhost/").is_err());
        assert!(parse_public_http_url("http://127.0.0.1/").is_err());
        assert!(parse_public_http_url("http://10.1.2.3/").is_err());
        assert!(parse_public_http_url("http://[::ffff:127.0.0.1]/").is_err());
        assert!(parse_public_http_url("file:///tmp/data.txt").is_err());
        assert!(parse_public_http_url("https://example.com/").is_ok());
    }

    #[test]
    fn web_fetch_allows_only_globally_routable_unicast_addresses() {
        for blocked in [
            "192.0.0.8",
            "192.0.2.1",
            "192.88.99.2",
            "198.18.0.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "2001:2::1",
            "2001:db8::1",
            "2002::1",
            "3fff::1",
            "64:ff9b::7f00:1",
            "64:ff9b::a9fe:a9fe",
            "100:0:0:1::1",
            "5f00::1",
            "fc00::1",
            "fe80::1",
            "fec0::1",
            "ff02::1",
        ] {
            let ip = blocked.parse::<IpAddr>().expect("test IP address");
            assert!(
                is_blocked_ip(ip),
                "{blocked} must not cross the public boundary"
            );
        }

        for public in [
            "93.184.216.34",
            "192.0.0.9",
            "2001:1::3",
            "64:ff9b::808:808",
            "2606:4700:4700::1111",
        ] {
            let ip = public.parse::<IpAddr>().expect("test IP address");
            assert!(!is_blocked_ip(ip), "{public} should remain fetchable");
        }
    }

    #[tokio::test]
    async fn web_fetch_rejects_special_use_dns_answers() {
        let url =
            parse_public_http_url("http://special-use.test/").expect("syntactically public URL");
        for blocked in ["198.18.0.1:80", "[fec0::1]:80"] {
            let resolver = ScriptedResolver::new([vec![
                blocked.parse::<SocketAddr>().expect("special-use address"),
            ]]);
            let error = resolve_public_target(&resolver, &url)
                .await
                .err()
                .expect("special-use DNS answer must fail closed");
            assert!(error.to_string().contains("non-public resolved address"));
            assert_eq!(resolver.calls(), 1);
        }
    }

    #[tokio::test]
    async fn web_fetch_total_timeout_bounds_stalled_headers() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("slow header listener");
        let address = listener.local_addr().expect("listener address");
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.expect("request connection");
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("test client");

        let error = with_fetch_timeout(Duration::from_millis(50), async {
            client
                .get(format!("http://{address}/"))
                .send()
                .await
                .map_err(reqwest_error)?;
            Ok(())
        })
        .await
        .expect_err("stalled response headers must time out");

        assert!(error.to_string().contains("web_fetch timed out"));
        server.abort();
    }

    #[tokio::test]
    async fn web_fetch_total_timeout_bounds_stalled_dns() {
        let backend = DirectHttpFetchBackend {
            resolver: Arc::new(HeldResolver),
            connect_timeout: Duration::from_secs(1),
            total_timeout: Duration::from_millis(50),
        };

        let error = backend
            .fetch("http://held-dns.test/", bt_core::WebFetchFormat::Text, 1024)
            .await
            .expect_err("stalled DNS must time out");

        assert!(error.to_string().contains("web_fetch timed out"));
    }

    #[tokio::test]
    async fn web_fetch_total_timeout_bounds_a_dripping_body() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("slow body listener");
        let address = listener.local_addr().expect("listener address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("request connection");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\na")
                .await
                .expect("partial response");
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("test client");

        let error = with_fetch_timeout(Duration::from_millis(50), async {
            let mut response = client
                .get(format!("http://{address}/"))
                .send()
                .await
                .map_err(reqwest_error)?;
            read_response_body_limited(&mut response, 1024).await?;
            Ok(())
        })
        .await
        .expect_err("dripping response body must time out");

        assert!(error.to_string().contains("web_fetch timed out"));
        server.abort();
    }

    #[tokio::test]
    async fn web_fetch_rejects_mixed_public_and_private_dns_answers() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("private listener");
        let private_addr = listener.local_addr().expect("listener address");
        let resolver =
            ScriptedResolver::new([vec![public_addr(private_addr.port()), private_addr]]);
        let url = parse_public_http_url(&format!(
            "http://mixed-answer.test:{}/",
            private_addr.port()
        ))
        .expect("syntactically public URL");

        let error = resolve_public_target(&resolver, &url)
            .await
            .err()
            .expect("mixed answer must fail closed");

        assert!(error.to_string().contains("non-public resolved address"));
        assert_eq!(resolver.calls(), 1);
        assert!(
            timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "validation must not contact an address from a mixed DNS answer"
        );
    }

    #[tokio::test]
    async fn web_fetch_rejects_ipv4_mapped_private_dns_answers() {
        let mapped_private = "[::ffff:127.0.0.1]:80"
            .parse::<SocketAddr>()
            .expect("mapped IPv4 address");
        let resolver = ScriptedResolver::new([vec![mapped_private]]);
        let url =
            parse_public_http_url("http://mapped-answer.test/").expect("syntactically public URL");

        let error = resolve_public_target(&resolver, &url)
            .await
            .err()
            .expect("mapped private answer must fail closed");

        assert!(error.to_string().contains("non-public resolved address"));
        assert_eq!(resolver.calls(), 1);
    }

    #[tokio::test]
    async fn web_fetch_rejects_nat64_embedded_private_dns_answers() {
        for blocked in ["[64:ff9b::7f00:1]:80", "[64:ff9b::a9fe:a9fe]:80"] {
            let resolver = ScriptedResolver::new([vec![
                blocked.parse::<SocketAddr>().expect("NAT64 address"),
            ]]);
            let url = parse_public_http_url("http://nat64-answer.test/")
                .expect("syntactically public URL");

            let error = resolve_public_target(&resolver, &url)
                .await
                .err()
                .expect("NAT64 address embedding a non-public IPv4 target must fail closed");

            assert!(error.to_string().contains("non-public resolved address"));
            assert_eq!(resolver.calls(), 1);
        }
    }

    #[tokio::test]
    async fn web_fetch_pins_the_first_dns_answer_instead_of_rebinding() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("private listener");
        let private_addr = listener.local_addr().expect("listener address");
        let resolver = Arc::new(ScriptedResolver::new([
            vec![public_addr(private_addr.port())],
            vec![private_addr],
        ]));
        let url = parse_public_http_url(&format!("http://rebind.test:{}/", private_addr.port()))
            .expect("syntactically public URL");

        let result =
            send_with_safe_redirects(resolver.as_ref(), url, Duration::from_millis(100)).await;

        assert!(
            result.is_err(),
            "the pinned public address is not a test server"
        );
        assert_eq!(
            resolver.calls(),
            1,
            "the HTTP connector must not perform a second DNS resolution"
        );
        assert!(
            timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "a rebinding answer must never reach the private listener"
        );
    }

    #[tokio::test]
    async fn pinned_connection_preserves_the_original_http_host() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("test listener");
        let local_addr = listener.local_addr().expect("listener address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("request connection");
            let mut request = vec![0_u8; 4096];
            let read = stream.read(&mut request).await.expect("HTTP request");
            let request = String::from_utf8_lossy(&request[..read]).into_owned();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .expect("HTTP response");
            request
        });
        let target = ResolvedTarget {
            host: "original-host.test".to_owned(),
            addrs: vec![local_addr],
            override_dns: true,
        };
        let client = pinned_client(&target, Duration::from_secs(1)).expect("pinned client");

        let response = client
            .get(format!(
                "http://original-host.test:{}/resource",
                local_addr.port()
            ))
            .send()
            .await
            .expect("pinned request");
        assert_eq!(response.status(), 200);
        let request = server.await.expect("server task");
        assert!(
            request.lines().any(|line| line
                .eq_ignore_ascii_case(&format!("host: original-host.test:{}", local_addr.port()))),
            "request must retain the URL hostname instead of substituting the pinned IP"
        );
    }

    #[tokio::test]
    async fn web_fetch_rejects_private_redirect_before_a_followup_request() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("private listener");
        let private_addr = listener.local_addr().expect("listener address");
        let current = parse_public_http_url("https://example.com/start").expect("public URL");

        let result = validated_redirect_url(
            &current,
            &format!("http://127.0.0.1:{}/secret", private_addr.port()),
        );

        assert!(result.is_err());
        assert!(
            timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "a private redirect target must be rejected before any request"
        );
    }

    #[test]
    fn exa_results_normalize_to_public_web_shape() {
        let payload = json!({
            "results": [
                {
                    "title": "Example",
                    "url": "https://example.com",
                    "text": "Example snippet",
                    "publishedDate": "2026-01-01",
                    "score": 0.42
                }
            ]
        });
        let response = normalize_exa_response("exa", "example query", 5, &payload);
        assert_eq!(response.backend, "exa");
        assert_eq!(response.query, "example query");
        assert_eq!(response.result_count, 1);
        assert_eq!(response.results[0].source, "exa");
        assert_eq!(response.results[0].url, "https://example.com");
        assert_eq!(
            response.results[0].snippet.as_deref(),
            Some("Example snippet")
        );
    }

    #[test]
    fn fetch_helpers_capture_title_and_stable_digest() {
        let html = "<html><head><title> Test &amp; Demo </title></head></html>";
        assert_eq!(extract_html_title(html).as_deref(), Some("Test & Demo"));
        assert_eq!(stable_content_digest(b"abc"), "fnv1a64:e71fa2190541574b");
    }

    #[test]
    fn direct_http_backend_is_named_for_traceability() {
        let backend = DirectHttpFetchBackend::new();
        assert_eq!(backend.name(), "direct_http");
    }

    #[test]
    fn fetch_body_limit_preserves_prefix_without_reading_past_cap() {
        let mut bytes = Vec::new();
        assert!(!append_limited_bytes(&mut bytes, b"abc", 5));
        assert!(append_limited_bytes(&mut bytes, b"def", 5));
        assert_eq!(bytes, b"abcde");
    }
}
