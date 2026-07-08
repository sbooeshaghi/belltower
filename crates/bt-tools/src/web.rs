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
use std::net::IpAddr;
use std::pin::Pin;
use tokio::net::lookup_host;

type BackendFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

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
            let backend = DirectHttpFetchBackend::new()?;
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

struct DirectHttpFetchBackend {
    client: Client,
}

impl DirectHttpFetchBackend {
    fn new() -> Result<Self> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(reqwest_error)?;
        Ok(Self { client })
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
            let mut response = send_with_safe_redirects(&self.client, requested.clone()).await?;
            let final_url = response.url().to_string();
            let status = response.status().as_u16();
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(ToOwned::to_owned);
            let (bytes, truncated_by_bytes) =
                read_response_body_limited(&mut response, max_bytes).await?;
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

async fn send_with_safe_redirects(client: &Client, initial_url: Url) -> Result<Response> {
    let mut current = initial_url;
    for _ in 0..=5 {
        ensure_public_url(&current).await?;
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
        current = current.join(location).map_err(|error| {
            BelltowerError::Tool(format!("invalid redirect location `{location}`: {error}"))
        })?;
        validate_url_host_without_dns(&current)?;
    }

    Err(BelltowerError::Tool(
        "too many redirects while fetching public URL".to_owned(),
    ))
}

async fn ensure_public_url(url: &Url) -> Result<()> {
    validate_url_host_without_dns(url)?;
    let host = url
        .host_str()
        .ok_or_else(|| BelltowerError::Tool("URL must include a host".to_owned()))?;
    if host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }

    let port = url.port_or_known_default().unwrap_or(443);
    let addrs = lookup_host((host, port)).await.map_err(|error| {
        BelltowerError::Tool(format!(
            "failed to resolve web_fetch host `{host}`: {error}"
        ))
    })?;
    for addr in addrs {
        if is_blocked_ip(addr.ip()) {
            return Err(BelltowerError::Tool(format!(
                "web_fetch blocked non-public resolved address `{}` for host `{host}`",
                addr.ip()
            )));
        }
    }
    Ok(())
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
    if let Ok(ip) = host.parse::<IpAddr>()
        && is_blocked_ip(ip)
    {
        return Err(BelltowerError::Tool(format!(
            "web_fetch blocked non-public address `{ip}`"
        )));
    }
    Ok(())
}

fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.octets()[0] == 0
                || (ip.octets()[0] == 100 && (ip.octets()[1] & 0b1100_0000) == 64)
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_multicast()
        }
    }
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
        DirectHttpFetchBackend, WebFetchBackend, WebFetchTool, WebSearchTool, append_limited_bytes,
        extract_html_title, normalize_exa_response, parse_public_http_url, stable_content_digest,
    };
    use bt_core::{ApprovalRequirement, ToolRiskClass, traits::ToolExecutor};
    use serde_json::json;

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
        assert!(parse_public_http_url("file:///tmp/data.txt").is_err());
        assert!(parse_public_http_url("https://example.com/").is_ok());
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
        let backend = DirectHttpFetchBackend::new().expect("backend");
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
