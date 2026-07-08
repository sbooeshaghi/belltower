use crate::{
    AuthMethodKind, BelltowerError, ConnectionAuthMethodDescriptor, ConnectionDescriptor,
    ConnectionId, ConnectionModelSelector, ModelBackendKind, Result,
};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::Path;
use url::Url;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub auth_token_path: Utf8PathBuf,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextConfig {
    pub compaction_threshold: f32,
    pub reserve_tokens: u64,
    pub max_tool_result_lines: usize,
    pub max_tool_result_bytes: usize,
    pub summary_connection: Option<ConnectionId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovalConfig {
    pub shell_timeout_seconds: u64,
    pub auto_approve_patterns: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DefaultsConfig {
    pub default_connection: ConnectionId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WebRetrievalConfig {
    pub search_backend: Option<String>,
    pub fetch_backend: String,
    pub backends: Vec<WebBackendConfig>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebBackendKind {
    Exa,
}

impl WebBackendKind {
    #[must_use]
    pub const fn provider_name(&self) -> &'static str {
        match self {
            Self::Exa => "exa",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WebBackendConfig {
    pub id: String,
    pub kind: WebBackendKind,
    pub enabled: bool,
    pub base_url: Url,
    #[serde(default)]
    pub auth_sources: Vec<String>,
}

impl WebBackendConfig {
    #[must_use]
    pub fn credential_provider_id(&self) -> String {
        format!("web:{}", self.id)
    }

    #[must_use]
    pub fn credential_descriptor(&self) -> ConnectionDescriptor {
        ConnectionDescriptor {
            id: ConnectionId::new(self.credential_provider_id()),
            provider: self.kind.provider_name().to_owned(),
            base_url: self.base_url.clone(),
            default_model: String::new(),
            auth_methods: vec![ConnectionAuthMethodDescriptor {
                id: format!("{}_api_key", self.id),
                kind: AuthMethodKind::ApiKey,
                label: format!("{} API key", self.kind.provider_name()),
                source_hint: None,
                supports_refresh: false,
            }],
            auth_sources: self.auth_sources.clone(),
            model_fallbacks: Vec::new(),
            discoverable_model_selectors: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelsConfig {
    pub request_timeout_ms: u64,
    pub backends: Vec<ModelBackendConfig>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelBackendConfig {
    pub kind: ModelBackendKind,
    pub enabled: bool,
    pub base_url: Url,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpConfig {
    pub servers: Vec<McpServerConfig>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub enabled: bool,
    pub transport: McpTransportConfig,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpTransportConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        cwd: Option<Utf8PathBuf>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    StreamableHttp {
        base_url: Url,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PricingCatalog {
    pub version: u32,
    pub pricing: Vec<PricingEntry>,
    #[serde(default)]
    pub unpriced_models: Vec<UnpricedModelEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PricingEntry {
    pub provider: String,
    pub model_pattern: String,
    pub prompt_usd_per_million: f64,
    pub completion_usd_per_million: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_usd_per_million: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_usd_per_million: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_usd_per_million: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UnpricedModelEntry {
    pub provider: String,
    pub model_pattern: String,
    /// Human-readable reason this model is intentionally absent from
    /// priced cost projections.
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextWindowCatalog {
    pub version: u32,
    pub window: Vec<ContextWindowEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextWindowEntry {
    pub provider: String,
    pub model_pattern: String,
    pub context_window_tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRecommendationCatalog {
    pub version: u32,
    pub tier: Vec<ModelRecommendationTier>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRecommendationTier {
    pub label: String,
    pub max_memory_gb: u64,
    pub models: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderCatalog {
    pub version: u32,
    pub connections: Vec<ConnectionFileConfig>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConnectionFileConfig {
    pub id: String,
    pub provider: String,
    pub base_url: String,
    pub default_model: String,
    #[serde(default)]
    pub auth_methods: Vec<ConnectionAuthMethodConfig>,
    #[serde(default)]
    pub auth_sources: Vec<String>,
    #[serde(default)]
    pub model_fallbacks: Vec<String>,
    #[serde(default)]
    pub discoverable_model_selectors: Vec<ConnectionModelSelector>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConnectionAuthMethodConfig {
    pub id: String,
    pub kind: AuthMethodKind,
    pub label: String,
    #[serde(default)]
    pub source_hint: Option<String>,
    #[serde(default)]
    pub supports_refresh: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BelltowerConfig {
    pub server: ServerConfig,
    pub context: ContextConfig,
    pub approval: ApprovalConfig,
    pub defaults: DefaultsConfig,
    pub web: WebRetrievalConfig,
    pub mcp: McpConfig,
    pub models: ModelsConfig,
    pub connections: Vec<ConnectionDescriptor>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct PartialServerConfig {
    host: Option<String>,
    port: Option<u16>,
    auth_token_path: Option<Utf8PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct PartialContextConfig {
    compaction_threshold: Option<f32>,
    reserve_tokens: Option<u64>,
    max_tool_result_lines: Option<usize>,
    max_tool_result_bytes: Option<usize>,
    summary_connection: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct PartialApprovalConfig {
    shell_timeout_seconds: Option<u64>,
    auto_approve_patterns: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct PartialDefaultsConfig {
    default_connection: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct PartialWebRetrievalConfig {
    search_backend: Option<String>,
    fetch_backend: Option<String>,
    backends: Option<BTreeMap<String, PartialWebBackendConfig>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct PartialWebBackendConfig {
    kind: Option<WebBackendKind>,
    enabled: Option<bool>,
    base_url: Option<String>,
    auth_sources: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct PartialModelsConfig {
    request_timeout_ms: Option<u64>,
    backends: Option<BTreeMap<String, PartialModelBackendConfig>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct PartialModelBackendConfig {
    enabled: Option<bool>,
    base_url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct PartialMcpServerConfig {
    enabled: Option<bool>,
    transport: Option<PartialMcpTransportConfig>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PartialMcpTransportConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        cwd: Option<Utf8PathBuf>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    StreamableHttp {
        base_url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct PartialConnectionDescriptor {
    provider: Option<String>,
    base_url: Option<String>,
    default_model: Option<String>,
    auth_methods: Option<Vec<ConnectionAuthMethodConfig>>,
    auth_sources: Option<Vec<String>>,
    model_fallbacks: Option<Vec<String>>,
    discoverable_model_selectors: Option<Vec<ConnectionModelSelector>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct PartialBelltowerConfig {
    server: Option<PartialServerConfig>,
    context: Option<PartialContextConfig>,
    approval: Option<PartialApprovalConfig>,
    defaults: Option<PartialDefaultsConfig>,
    web: Option<PartialWebRetrievalConfig>,
    mcp: Option<BTreeMap<String, PartialMcpServerConfig>>,
    models: Option<PartialModelsConfig>,
    connections: Option<BTreeMap<String, PartialConnectionDescriptor>>,
}

impl BelltowerConfig {
    pub fn load(project_root: Option<&Utf8Path>) -> Result<Self> {
        let mut config = Self::from_embedded()?;
        let global_path = config_path();
        if global_path.exists() {
            config.apply_overrides(&fs::read_to_string(&global_path)?)?;
        }

        if let Some(project_root) = project_root {
            let project_path = project_root.join(".belltower/config.toml");
            if project_path.exists() {
                config.apply_overrides(&fs::read_to_string(project_path)?)?;
            }
        }

        Ok(config)
    }

    pub fn from_embedded() -> Result<Self> {
        let provider_catalog: ProviderCatalog =
            toml::from_str(include_str!("../data/providers/catalog.toml"))?;
        let descriptors = provider_catalog
            .connections
            .into_iter()
            .map(ConnectionDescriptor::try_from)
            .collect::<Result<Vec<_>>>()?;

        Ok(Self {
            server: ServerConfig {
                host: "127.0.0.1".to_owned(),
                port: 7400,
                auth_token_path: auth_token_path(),
            },
            context: ContextConfig {
                compaction_threshold: 0.75,
                reserve_tokens: 16_384,
                max_tool_result_lines: 2_000,
                max_tool_result_bytes: 51_200,
                summary_connection: Some(ConnectionId::new("local")),
            },
            approval: ApprovalConfig {
                shell_timeout_seconds: 120,
                auto_approve_patterns: vec![
                    "cargo build".to_owned(),
                    "cargo test".to_owned(),
                    "cargo clippy".to_owned(),
                    "python -m pytest".to_owned(),
                ],
            },
            defaults: DefaultsConfig {
                default_connection: ConnectionId::new("local"),
            },
            web: WebRetrievalConfig {
                search_backend: None,
                fetch_backend: "direct_http".to_owned(),
                backends: vec![WebBackendConfig {
                    id: "exa".to_owned(),
                    kind: WebBackendKind::Exa,
                    enabled: true,
                    base_url: Url::parse("https://api.exa.ai")?,
                    auth_sources: vec![
                        "env:EXA_API_KEY".to_owned(),
                        "env:BELLTOWER_EXA_API_KEY".to_owned(),
                    ],
                }],
            },
            mcp: McpConfig {
                servers: Vec::new(),
            },
            models: ModelsConfig {
                request_timeout_ms: 1_000,
                backends: vec![
                    ModelBackendConfig {
                        kind: ModelBackendKind::Ollama,
                        enabled: true,
                        base_url: Url::parse("http://127.0.0.1:11434")?,
                    },
                    ModelBackendConfig {
                        kind: ModelBackendKind::LmStudio,
                        enabled: true,
                        base_url: Url::parse("http://127.0.0.1:1234")?,
                    },
                    ModelBackendConfig {
                        kind: ModelBackendKind::LlamaCpp,
                        enabled: true,
                        base_url: Url::parse("http://127.0.0.1:8080")?,
                    },
                ],
            },
            connections: descriptors,
        })
    }

    pub fn pricing_catalog() -> Result<PricingCatalog> {
        Ok(toml::from_str(include_str!(
            "../data/providers/pricing.toml"
        ))?)
    }

    pub fn context_window_catalog() -> Result<ContextWindowCatalog> {
        Ok(toml::from_str(include_str!(
            "../data/providers/context_windows.toml"
        ))?)
    }

    pub fn model_recommendations() -> Result<ModelRecommendationCatalog> {
        Ok(toml::from_str(include_str!(
            "../data/models/recommendations.toml"
        ))?)
    }

    fn apply_overrides(&mut self, raw_toml: &str) -> Result<()> {
        let partial: PartialBelltowerConfig = toml::from_str(raw_toml)?;

        if let Some(server) = partial.server {
            if let Some(host) = server.host {
                self.server.host = host;
            }
            if let Some(port) = server.port {
                self.server.port = port;
            }
            if let Some(path) = server.auth_token_path {
                self.server.auth_token_path = path;
            }
        }

        if let Some(context) = partial.context {
            if let Some(threshold) = context.compaction_threshold {
                self.context.compaction_threshold = threshold;
            }
            if let Some(tokens) = context.reserve_tokens {
                self.context.reserve_tokens = tokens;
            }
            if let Some(lines) = context.max_tool_result_lines {
                self.context.max_tool_result_lines = lines;
            }
            if let Some(bytes) = context.max_tool_result_bytes {
                self.context.max_tool_result_bytes = bytes;
            }
            if let Some(summary_connection) = context.summary_connection {
                self.context.summary_connection = Some(ConnectionId::new(summary_connection));
            }
        }

        if let Some(approval) = partial.approval {
            if let Some(timeout) = approval.shell_timeout_seconds {
                self.approval.shell_timeout_seconds = timeout;
            }
            if let Some(patterns) = approval.auto_approve_patterns {
                self.approval.auto_approve_patterns = patterns;
            }
        }

        if let Some(defaults) = partial.defaults
            && let Some(connection) = defaults.default_connection
        {
            self.defaults.default_connection = ConnectionId::new(connection);
        }

        if let Some(web) = partial.web {
            if let Some(search_backend) = web.search_backend {
                self.web.search_backend = trimmed_optional_string(search_backend);
            }
            if let Some(fetch_backend) = web.fetch_backend {
                let fetch_backend = fetch_backend.trim();
                if fetch_backend.is_empty() {
                    return Err(BelltowerError::Config(
                        "web.fetch_backend cannot be empty".to_owned(),
                    ));
                }
                self.web.fetch_backend = fetch_backend.to_owned();
            }
            if let Some(backends) = web.backends {
                for (id, override_entry) in backends {
                    let backend = self
                        .web
                        .backends
                        .iter_mut()
                        .find(|backend| backend.id == id);
                    if let Some(backend) = backend {
                        if let Some(kind) = override_entry.kind {
                            backend.kind = kind;
                        }
                        if let Some(enabled) = override_entry.enabled {
                            backend.enabled = enabled;
                        }
                        if let Some(base_url) = override_entry.base_url {
                            backend.base_url = Url::parse(&base_url)?;
                        }
                        if let Some(auth_sources) = override_entry.auth_sources {
                            backend.auth_sources = auth_sources;
                        }
                    } else {
                        let kind = override_entry.kind.ok_or_else(|| {
                            BelltowerError::Config(format!(
                                "new web backend override `{id}` is missing `kind`"
                            ))
                        })?;
                        let base_url = override_entry.base_url.ok_or_else(|| {
                            BelltowerError::Config(format!(
                                "new web backend override `{id}` is missing `base_url`"
                            ))
                        })?;
                        self.web.backends.push(WebBackendConfig {
                            id,
                            kind,
                            enabled: override_entry.enabled.unwrap_or(true),
                            base_url: Url::parse(&base_url)?,
                            auth_sources: override_entry.auth_sources.unwrap_or_default(),
                        });
                    }
                }
            }
        }

        if let Some(mcp_servers) = partial.mcp {
            for (name, override_entry) in mcp_servers {
                let server = self
                    .mcp
                    .servers
                    .iter_mut()
                    .find(|server| server.name == name);
                if let Some(server) = server {
                    if let Some(enabled) = override_entry.enabled {
                        server.enabled = enabled;
                    }
                    if let Some(transport) = override_entry.transport {
                        server.transport = materialize_partial_mcp_transport(transport)?;
                    }
                } else {
                    let transport = override_entry.transport.ok_or_else(|| {
                        BelltowerError::Config(format!(
                            "new MCP server override `{name}` is missing `transport`"
                        ))
                    })?;
                    self.mcp.servers.push(McpServerConfig {
                        name,
                        enabled: override_entry.enabled.unwrap_or(true),
                        transport: materialize_partial_mcp_transport(transport)?,
                    });
                }
            }
        }

        if let Some(models) = partial.models {
            if let Some(timeout) = models.request_timeout_ms {
                self.models.request_timeout_ms = timeout;
            }
            if let Some(backends) = models.backends {
                for (kind_name, override_entry) in backends {
                    let kind = parse_model_backend_kind(&kind_name)?;
                    let backend = self
                        .models
                        .backends
                        .iter_mut()
                        .find(|backend| backend.kind == kind);
                    if let Some(backend) = backend {
                        if let Some(enabled) = override_entry.enabled {
                            backend.enabled = enabled;
                        }
                        if let Some(base_url) = override_entry.base_url {
                            backend.base_url = Url::parse(&base_url)?;
                        }
                    } else {
                        let base_url = override_entry.base_url.ok_or_else(|| {
                            BelltowerError::Config(format!(
                                "new model backend override `{kind_name}` is missing `base_url`"
                            ))
                        })?;
                        self.models.backends.push(ModelBackendConfig {
                            kind,
                            enabled: override_entry.enabled.unwrap_or(true),
                            base_url: Url::parse(&base_url)?,
                        });
                    }
                }
            }
        }

        if let Some(connections) = partial.connections {
            for (id, override_entry) in connections {
                let descriptor = self
                    .connections
                    .iter_mut()
                    .find(|descriptor| descriptor.id.0 == id);

                if let Some(descriptor) = descriptor {
                    if let Some(provider) = override_entry.provider {
                        descriptor.provider = provider;
                    }
                    if let Some(base_url) = override_entry.base_url {
                        descriptor.base_url = Url::parse(&base_url)?;
                    }
                    if let Some(default_model) = override_entry.default_model {
                        descriptor.default_model = default_model;
                    }
                    if let Some(auth_methods) = override_entry.auth_methods {
                        descriptor.auth_methods = auth_methods
                            .into_iter()
                            .map(ConnectionAuthMethodDescriptor::from)
                            .collect();
                    }
                    if let Some(auth_sources) = override_entry.auth_sources {
                        descriptor.auth_sources = auth_sources;
                    }
                    if let Some(model_fallbacks) = override_entry.model_fallbacks {
                        descriptor.model_fallbacks = model_fallbacks;
                    }
                    if let Some(discoverable_model_selectors) =
                        override_entry.discoverable_model_selectors
                    {
                        descriptor.discoverable_model_selectors = discoverable_model_selectors;
                    }
                } else {
                    let provider = override_entry.provider.ok_or_else(|| {
                        BelltowerError::Config(format!(
                            "new connection override `{id}` is missing `provider`"
                        ))
                    })?;
                    let base_url = override_entry.base_url.ok_or_else(|| {
                        BelltowerError::Config(format!(
                            "new connection override `{id}` is missing `base_url`"
                        ))
                    })?;
                    let default_model = override_entry.default_model.ok_or_else(|| {
                        BelltowerError::Config(format!(
                            "new connection override `{id}` is missing `default_model`"
                        ))
                    })?;
                    self.connections.push(ConnectionDescriptor {
                        id: ConnectionId::new(id),
                        provider,
                        base_url: Url::parse(&base_url)?,
                        default_model,
                        auth_methods: override_entry
                            .auth_methods
                            .unwrap_or_default()
                            .into_iter()
                            .map(ConnectionAuthMethodDescriptor::from)
                            .collect(),
                        auth_sources: override_entry.auth_sources.unwrap_or_default(),
                        model_fallbacks: override_entry.model_fallbacks.unwrap_or_default(),
                        discoverable_model_selectors: override_entry
                            .discoverable_model_selectors
                            .unwrap_or_default(),
                    });
                }
            }
        }

        Ok(())
    }
}

fn trimmed_optional_string(value: String) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

impl TryFrom<ConnectionFileConfig> for ConnectionDescriptor {
    type Error = BelltowerError;

    fn try_from(value: ConnectionFileConfig) -> Result<Self> {
        Ok(Self {
            id: ConnectionId::new(value.id),
            provider: value.provider,
            base_url: Url::parse(&value.base_url)?,
            default_model: value.default_model,
            auth_methods: value
                .auth_methods
                .into_iter()
                .map(ConnectionAuthMethodDescriptor::from)
                .collect(),
            auth_sources: value.auth_sources,
            model_fallbacks: value.model_fallbacks,
            discoverable_model_selectors: value.discoverable_model_selectors,
        })
    }
}

impl From<ConnectionAuthMethodConfig> for ConnectionAuthMethodDescriptor {
    fn from(value: ConnectionAuthMethodConfig) -> Self {
        Self {
            id: value.id,
            kind: value.kind,
            label: value.label,
            source_hint: value.source_hint,
            supports_refresh: value.supports_refresh,
        }
    }
}

fn materialize_partial_mcp_transport(
    transport: PartialMcpTransportConfig,
) -> Result<McpTransportConfig> {
    match transport {
        PartialMcpTransportConfig::Stdio {
            command,
            args,
            cwd,
            env,
        } => Ok(McpTransportConfig::Stdio {
            command,
            args,
            cwd,
            env,
        }),
        PartialMcpTransportConfig::StreamableHttp { base_url, headers } => {
            Ok(McpTransportConfig::StreamableHttp {
                base_url: Url::parse(&base_url)?,
                headers,
            })
        }
    }
}

fn parse_model_backend_kind(raw: &str) -> Result<ModelBackendKind> {
    match raw {
        "ollama" => Ok(ModelBackendKind::Ollama),
        "lm_studio" => Ok(ModelBackendKind::LmStudio),
        "llama_cpp" => Ok(ModelBackendKind::LlamaCpp),
        other => Err(BelltowerError::Config(format!(
            "unknown model backend `{other}`"
        ))),
    }
}

#[must_use]
pub fn config_dir() -> Utf8PathBuf {
    if let Ok(override_dir) = env::var("BELLTOWER_CONFIG_DIR") {
        return Utf8PathBuf::from(override_dir);
    }

    if let Ok(xdg) = env::var("XDG_CONFIG_HOME") {
        return Utf8PathBuf::from(xdg).join("belltower");
    }

    if let Ok(home) = env::var("HOME") {
        return Utf8PathBuf::from(home).join(".config/belltower");
    }

    Utf8PathBuf::from(".belltower")
}

#[must_use]
pub fn config_path() -> Utf8PathBuf {
    config_dir().join("config.toml")
}

pub fn persist_default_connection(connection_id: &ConnectionId) -> Result<()> {
    persist_default_connection_at(config_path().as_std_path(), connection_id)
}

pub fn persist_connection_default_model(connection_id: &ConnectionId, model: &str) -> Result<()> {
    persist_connection_default_model_at(config_path().as_std_path(), connection_id, model)
}

pub fn persist_web_search_backend(backend_id: &str) -> Result<()> {
    persist_web_search_backend_at(config_path().as_std_path(), backend_id)
}

fn persist_default_connection_at(path: &Path, connection_id: &ConnectionId) -> Result<()> {
    persist_config_update_at(path, |root| {
        let defaults = root
            .entry("defaults".to_owned())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        let defaults = defaults
            .as_table_mut()
            .ok_or_else(|| BelltowerError::Config("defaults config must be a table".to_owned()))?;
        defaults.insert(
            "default_connection".to_owned(),
            toml::Value::String(connection_id.to_string()),
        );
        Ok(())
    })
}

fn persist_connection_default_model_at(
    path: &Path,
    connection_id: &ConnectionId,
    model: &str,
) -> Result<()> {
    persist_config_update_at(path, |root| {
        let connections = root
            .entry("connections".to_owned())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        let connections = connections.as_table_mut().ok_or_else(|| {
            BelltowerError::Config("connections config must be a table".to_owned())
        })?;
        let connection = connections
            .entry(connection_id.to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        let connection = connection.as_table_mut().ok_or_else(|| {
            BelltowerError::Config("connection override must be a table".to_owned())
        })?;
        connection.insert(
            "default_model".to_owned(),
            toml::Value::String(model.to_owned()),
        );
        Ok(())
    })
}

fn persist_web_search_backend_at(path: &Path, backend_id: &str) -> Result<()> {
    let backend_id = backend_id.trim();
    if backend_id.is_empty() {
        return Err(BelltowerError::Config(
            "web search backend cannot be empty".to_owned(),
        ));
    }
    persist_config_update_at(path, |root| {
        let web = root
            .entry("web".to_owned())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        let web = web
            .as_table_mut()
            .ok_or_else(|| BelltowerError::Config("web config must be a table".to_owned()))?;
        web.insert(
            "search_backend".to_owned(),
            toml::Value::String(backend_id.to_owned()),
        );
        Ok(())
    })
}

fn persist_config_update_at(
    path: &Path,
    update: impl FnOnce(&mut toml::map::Map<String, toml::Value>) -> Result<()>,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut value = if path.exists() {
        toml::from_str::<toml::Value>(&fs::read_to_string(path)?)?
    } else {
        toml::Value::Table(toml::map::Map::new())
    };
    if !value.is_table() {
        value = toml::Value::Table(toml::map::Map::new());
    }
    let root = value.as_table_mut().expect("table just created");
    update(root)?;
    fs::write(path, toml::to_string_pretty(&value)?)?;
    Ok(())
}

#[must_use]
pub fn data_dir() -> Utf8PathBuf {
    if let Ok(override_dir) = env::var("BELLTOWER_DATA_DIR") {
        return Utf8PathBuf::from(override_dir);
    }

    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data")
}

#[must_use]
pub fn auth_dir() -> Utf8PathBuf {
    config_dir().join("auth")
}

#[must_use]
pub fn auth_store_path() -> Utf8PathBuf {
    config_dir().join("auth.json")
}

#[must_use]
pub fn auth_token_path() -> Utf8PathBuf {
    config_dir().join("auth_token")
}

#[must_use]
pub fn local_server_auth_dir() -> Utf8PathBuf {
    config_dir().join("server-auth")
}

pub fn server_auth_token_path_for_url(server_url: &Url) -> Result<Utf8PathBuf> {
    let host = server_url.host_str().ok_or_else(|| {
        BelltowerError::Config(format!("server url `{server_url}` is missing a host"))
    })?;
    let port = server_url.port_or_known_default().ok_or_else(|| {
        BelltowerError::Config(format!("server url `{server_url}` is missing a port"))
    })?;
    let scheme = sanitize_server_auth_component(server_url.scheme());
    let host = sanitize_server_auth_component(host);
    Ok(local_server_auth_dir().join(format!("{scheme}_{host}_{port}.token")))
}

fn sanitize_server_auth_component(raw: &str) -> String {
    raw.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        BelltowerConfig, persist_connection_default_model_at, persist_default_connection_at,
        persist_web_search_backend_at,
    };
    use crate::{ConnectionId, ModelPricing};
    use std::fs;
    use url::Url;

    #[test]
    fn embedded_catalog_loads() {
        let config = BelltowerConfig::from_embedded().expect("embedded config should load");
        assert!(!config.connections.is_empty());
        assert!(config.mcp.servers.is_empty());
        assert_eq!(config.models.backends.len(), 3);
        assert_eq!(
            config.defaults.default_connection,
            ConnectionId::new("local")
        );
        let openai = config
            .connections
            .iter()
            .find(|connection| connection.id == ConnectionId::new("openai"))
            .expect("openai connection");
        assert_eq!(openai.auth_methods.len(), 1);
        assert_eq!(openai.auth_methods[0].id, "openai_api_key");
        let chatgpt = config
            .connections
            .iter()
            .find(|connection| connection.id == ConnectionId::new("chatgpt"))
            .expect("chatgpt connection");
        assert_eq!(chatgpt.provider, "openai-chatgpt");
        assert_eq!(chatgpt.auth_methods.len(), 1);
        assert_eq!(chatgpt.auth_methods[0].id, "chatgpt_device_code");
        assert_eq!(config.web.fetch_backend, "direct_http");
        let exa = config
            .web
            .backends
            .iter()
            .find(|backend| backend.id == "exa")
            .expect("exa web backend");
        assert!(exa.enabled);
        assert_eq!(exa.kind.provider_name(), "exa");
        assert_eq!(exa.credential_provider_id(), "web:exa");
        assert_eq!(exa.credential_descriptor().auth_sources, exa.auth_sources);
    }

    #[test]
    fn embedded_tables_load() {
        assert!(BelltowerConfig::pricing_catalog().is_ok());
        assert!(BelltowerConfig::context_window_catalog().is_ok());
        assert!(BelltowerConfig::model_recommendations().is_ok());
    }

    #[test]
    fn pricing_catalog_supports_token_classes_and_unpriced_models() {
        let catalog: super::PricingCatalog = toml::from_str(
            r#"
version = 1

[[pricing]]
provider = "openai"
model_pattern = "gpt-5.4"
prompt_usd_per_million = 1.0
completion_usd_per_million = 2.0
cache_read_usd_per_million = 0.1
cache_write_usd_per_million = 0.2
reasoning_usd_per_million = 3.0

[[unpriced_models]]
provider = "openai-chatgpt"
model_pattern = "gpt-5.4-pro"
reason = "subscription-backed model has no public API price"
"#,
        )
        .expect("pricing catalog");

        let priced = catalog.pricing.first().expect("priced entry");
        assert_eq!(priced.cache_read_usd_per_million, Some(0.1));
        assert_eq!(priced.cache_write_usd_per_million, Some(0.2));
        assert_eq!(priced.reasoning_usd_per_million, Some(3.0));
        let unpriced = catalog
            .unpriced_models
            .first()
            .expect("unpriced model entry");
        assert_eq!(unpriced.provider, "openai-chatgpt");
        assert_eq!(
            unpriced.reason,
            "subscription-backed model has no public API price"
        );
    }

    #[test]
    fn model_pricing_shape_can_represent_token_class_rates_and_unpriced_reason() {
        let priced = ModelPricing {
            provider: "openai".to_owned(),
            model_pattern: "gpt-5.4".to_owned(),
            prompt_usd_per_million: 1.0,
            completion_usd_per_million: 2.0,
            cache_read_usd_per_million: Some(0.1),
            cache_write_usd_per_million: Some(0.2),
            reasoning_usd_per_million: Some(3.0),
            unpriced_reason: None,
        };
        let unpriced = ModelPricing {
            provider: "openai-chatgpt".to_owned(),
            model_pattern: "gpt-5.4-pro".to_owned(),
            prompt_usd_per_million: 0.0,
            completion_usd_per_million: 0.0,
            cache_read_usd_per_million: None,
            cache_write_usd_per_million: None,
            reasoning_usd_per_million: None,
            unpriced_reason: Some("subscription-backed model has no public API price".to_owned()),
        };

        assert_eq!(priced.cache_read_usd_per_million, Some(0.1));
        assert_eq!(priced.cache_write_usd_per_million, Some(0.2));
        assert_eq!(priced.reasoning_usd_per_million, Some(3.0));
        assert_eq!(
            unpriced.unpriced_reason.as_deref(),
            Some("subscription-backed model has no public API price")
        );
    }

    #[test]
    fn server_auth_token_path_is_endpoint_scoped() {
        let first = super::server_auth_token_path_for_url(
            &Url::parse("http://127.0.0.1:7400/").expect("url"),
        )
        .expect("path");
        let second = super::server_auth_token_path_for_url(
            &Url::parse("http://127.0.0.1:7401/").expect("url"),
        )
        .expect("path");

        assert_ne!(first, second);
        assert!(first.as_str().contains("127_0_0_1_7400"));
        assert!(second.as_str().contains("127_0_0_1_7401"));
    }

    #[test]
    fn persist_default_connection_creates_nested_tables() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config_path = temp_dir.path().join("config.toml");
        persist_default_connection_at(&config_path, &ConnectionId::new("openai"))
            .expect("config write should succeed");
        let written = fs::read_to_string(config_path).expect("config should exist");
        assert!(written.contains("[defaults]"));
        assert!(written.contains("default_connection = \"openai\""));
    }

    #[test]
    fn persist_connection_default_model_creates_connection_overrides() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config_path = temp_dir.path().join("config.toml");
        persist_connection_default_model_at(&config_path, &ConnectionId::new("openai"), "gpt-5.1")
            .expect("config write should succeed");
        let written = fs::read_to_string(config_path).expect("config should exist");
        assert!(written.contains("[connections.openai]"));
        assert!(written.contains("default_model = \"gpt-5.1\""));
    }

    #[test]
    fn persist_web_search_backend_creates_web_override() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config_path = temp_dir.path().join("config.toml");
        persist_web_search_backend_at(&config_path, "exa").expect("config write should succeed");
        let written = fs::read_to_string(config_path).expect("config should exist");
        assert!(written.contains("[web]"));
        assert!(written.contains("search_backend = \"exa\""));
    }

    #[test]
    fn web_config_override_can_set_search_backend_and_sources() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let project_root =
            camino::Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf()).expect("utf8 path");
        let config_path = project_root.join(".belltower/config.toml");
        fs::create_dir_all(config_path.parent().expect("config parent")).expect("config dir");
        fs::write(
            &config_path,
            r#"
[web]
search_backend = "exa"

[web.backends.exa]
auth_sources = ["env:CUSTOM_EXA_KEY"]
"#,
        )
        .expect("write config");
        let config = BelltowerConfig::load(Some(&project_root)).expect("config");
        assert_eq!(config.web.search_backend.as_deref(), Some("exa"));
        let exa = config
            .web
            .backends
            .iter()
            .find(|backend| backend.id == "exa")
            .expect("exa backend");
        assert_eq!(exa.auth_sources, vec!["env:CUSTOM_EXA_KEY"]);
    }
}
