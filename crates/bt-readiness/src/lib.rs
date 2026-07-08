#![forbid(unsafe_code)]

use bt_auth::{CredentialResolver, ResolvedCredential, auth_storage_summary};
use bt_core::{
    AuthMethodKind, BelltowerConfig, BelltowerError, ConnectionAuthState, ConnectionDescriptor,
    ConnectionModelInventory, ConnectionModelOption, ConnectionModelSource,
    ConnectionReadinessInspection, ConnectionReadinessState, ConnectionStatus,
    ConnectionSupportState, CredentialKind, ModelBackendDescriptor, ModelBackendStatus, Result,
    StatusInspection, WebBackendConfig, WebBackendReadinessInspection, WebBackendReadinessState,
    WebRetrievalInspection,
};
use bt_models::LocalModelManager;
use bt_providers::{connection_supported, provider_for_connection};
use std::collections::BTreeSet;
use std::time::Duration;
use url::Url;

pub async fn inspect_status(config: &BelltowerConfig) -> Result<StatusInspection> {
    let resolver = CredentialResolver::new()?;
    let mut connections = Vec::with_capacity(config.connections.len());
    for connection in &config.connections {
        connections.push(inspect_connection(config, connection, &resolver).await?);
    }
    Ok(StatusInspection {
        auth_storage: auth_storage_summary(None)?,
        default_connection: config.defaults.default_connection.clone(),
        connections,
    })
}

pub fn inspect_web_status(config: &BelltowerConfig) -> Result<WebRetrievalInspection> {
    let resolver = CredentialResolver::new()?;
    Ok(WebRetrievalInspection {
        search_backend: config.web.search_backend.clone(),
        fetch_backend: config.web.fetch_backend.clone(),
        backends: config
            .web
            .backends
            .iter()
            .map(|backend| inspect_web_backend(backend, &resolver))
            .collect::<Result<Vec<_>>>()?,
    })
}

pub async fn inspect_connection_status(
    config: &BelltowerConfig,
    connection_id: &bt_core::ConnectionId,
) -> Result<ConnectionReadinessInspection> {
    let connection = find_connection(config, connection_id)?;
    let resolver = CredentialResolver::new()?;
    inspect_connection(config, connection, &resolver).await
}

pub async fn inspect_connection_status_with_timeout(
    config: &BelltowerConfig,
    connection_id: &bt_core::ConnectionId,
    timeout: Duration,
) -> Result<ConnectionReadinessInspection> {
    let connection = find_connection(config, connection_id)?;
    match tokio::time::timeout(timeout, async {
        let resolver = CredentialResolver::new()?;
        inspect_connection(config, connection, &resolver).await
    })
    .await
    {
        Ok(inspection) => inspection,
        Err(_) => Ok(connection_timeout_inspection(connection, timeout)),
    }
}

pub async fn inspect_connection_models(
    config: &BelltowerConfig,
) -> Result<Vec<ConnectionModelInventory>> {
    let resolver = CredentialResolver::new()?;
    let mut inventories = Vec::with_capacity(config.connections.len());
    for connection in &config.connections {
        inventories
            .push(inspect_connection_model_inventory_inner(config, connection, &resolver).await?);
    }
    Ok(inventories)
}

pub async fn inspect_connection_model_inventory(
    config: &BelltowerConfig,
    connection_id: &bt_core::ConnectionId,
) -> Result<ConnectionModelInventory> {
    let connection = find_connection(config, connection_id)?;
    let resolver = CredentialResolver::new()?;
    inspect_connection_model_inventory_inner(config, connection, &resolver).await
}

fn find_connection<'a>(
    config: &'a BelltowerConfig,
    connection_id: &bt_core::ConnectionId,
) -> Result<&'a ConnectionDescriptor> {
    config
        .connections
        .iter()
        .find(|connection| &connection.id == connection_id)
        .ok_or_else(|| BelltowerError::Config(format!("connection `{connection_id}` not found")))
}

async fn inspect_connection_model_inventory_inner(
    config: &BelltowerConfig,
    connection: &ConnectionDescriptor,
    resolver: &CredentialResolver,
) -> Result<ConnectionModelInventory> {
    let credential = resolve_credential_for_readiness(resolver, connection).await;
    if should_probe_remote_models_once(connection, &credential) {
        return inspect_remote_connection_model_inventory_once(config, connection, credential)
            .await;
    }

    let inspection =
        inspect_connection_with_credential(config, connection, credential.clone()).await?;
    let (discovered_source, discovered_models) = if credential.error.is_some() {
        (None, Vec::new())
    } else {
        discover_connection_models(config, connection, credential.credential.as_ref()).await?
    };
    Ok(build_connection_model_inventory(
        connection,
        inspection,
        discovered_source,
        discovered_models,
    ))
}

fn should_probe_remote_models_once(
    connection: &ConnectionDescriptor,
    credential: &ReadinessCredential,
) -> bool {
    connection.id != bt_core::ConnectionId::new("local")
        && connection_supported(connection)
        && credential.error.is_none()
        && (connection.auth_sources.is_empty() || credential.credential.is_some())
        && credential
            .credential
            .as_ref()
            .is_none_or(|credential| credential_supported_by_connection(connection, credential))
}

async fn inspect_remote_connection_model_inventory_once(
    config: &BelltowerConfig,
    connection: &ConnectionDescriptor,
    credential: ReadinessCredential,
) -> Result<ConnectionModelInventory> {
    let timeout = Duration::from_millis(config.models.request_timeout_ms.max(1_000));
    let source_suffix = credential
        .credential
        .as_ref()
        .map(|credential| format!(" via {}", credential_summary(credential)))
        .unwrap_or_default();

    let discovery = tokio::time::timeout(
        timeout,
        try_discover_connection_models(config, connection, credential.credential.as_ref()),
    )
    .await;

    let (probe, discovered_source, discovered_models) = match discovery {
        Ok(Ok((source, models))) => {
            let probe =
                remote_model_discovery_probe(connection, source.as_deref(), &models, source_suffix);
            (probe, source, models)
        }
        Ok(Err(error)) => (
            ConnectionProbe {
                state: ConnectionReadinessState::ValidationFailed,
                ready: false,
                status: format!("validation failed: {error}{source_suffix}"),
            },
            None,
            Vec::new(),
        ),
        Err(_) => (
            ConnectionProbe {
                state: ConnectionReadinessState::Unreachable,
                ready: false,
                status: format!(
                    "unreachable: timed out after {} ms{source_suffix}",
                    timeout.as_millis()
                ),
            },
            None,
            Vec::new(),
        ),
    };

    let inspection = connection_inspection_from_probe(connection, &credential, probe);
    Ok(build_connection_model_inventory(
        connection,
        inspection,
        discovered_source,
        discovered_models,
    ))
}

fn remote_model_discovery_probe(
    connection: &ConnectionDescriptor,
    discovered_source: Option<&str>,
    raw_discovered_models: &[String],
    source_suffix: String,
) -> ConnectionProbe {
    if raw_discovered_models.is_empty() {
        return ConnectionProbe {
            state: ConnectionReadinessState::Ready,
            ready: true,
            status: format!("healthy{source_suffix}"),
        };
    }

    let discovered_models = curated_connection_models(connection, raw_discovered_models.to_vec());
    if model_is_available_for_connection(connection, &discovered_models, &connection.default_model)
    {
        return ConnectionProbe {
            state: ConnectionReadinessState::Ready,
            ready: true,
            status: format!("healthy{source_suffix}"),
        };
    }

    let discovered_source = discovered_source.unwrap_or(&connection.provider);
    ConnectionProbe {
        state: ConnectionReadinessState::ConfiguredModelUnavailable,
        ready: false,
        status: format!(
            "healthy{source_suffix}, but configured model `{}` is not available on {}",
            connection.default_model, discovered_source
        ),
    }
}

async fn inspect_connection(
    config: &BelltowerConfig,
    connection: &ConnectionDescriptor,
    resolver: &CredentialResolver,
) -> Result<ConnectionReadinessInspection> {
    let credential = resolve_credential_for_readiness(resolver, connection).await;
    inspect_connection_with_credential(config, connection, credential).await
}

async fn inspect_connection_with_credential(
    config: &BelltowerConfig,
    connection: &ConnectionDescriptor,
    credential: ReadinessCredential,
) -> Result<ConnectionReadinessInspection> {
    let probe = if let Some(error) = credential.error.as_deref() {
        credential_resolution_failure_probe(error)
    } else {
        probe_connection_readiness(config, connection, credential.credential.as_ref()).await?
    };

    Ok(connection_inspection_from_probe(
        connection,
        &credential,
        probe,
    ))
}

fn connection_inspection_from_probe(
    connection: &ConnectionDescriptor,
    credential: &ReadinessCredential,
    probe: ConnectionProbe,
) -> ConnectionReadinessInspection {
    let (auth_state, auth_kind, auth_source, auth_tried_sources) =
        if connection.auth_sources.is_empty() {
            (ConnectionAuthState::NotRequired, None, None, Vec::new())
        } else if let Some(credential) = credential.credential.as_ref() {
            (
                ConnectionAuthState::Configured,
                Some(credential.kind.clone()),
                Some(credential.source.clone()),
                credential.tried_sources.clone(),
            )
        } else {
            (ConnectionAuthState::Missing, None, None, Vec::new())
        };

    ConnectionReadinessInspection {
        connection_id: connection.id.clone(),
        provider: connection.provider.clone(),
        default_model: connection.default_model.clone(),
        auth_methods: connection.auth_methods.clone(),
        auth_state,
        auth_kind,
        auth_source,
        auth_tried_sources,
        support_state: connection_support_state(connection),
        readiness_state: probe.state,
        probe_ready: probe.ready,
        probe_status: probe.status,
    }
}

fn connection_support_state(connection: &ConnectionDescriptor) -> ConnectionSupportState {
    if connection_supported(connection) {
        ConnectionSupportState::RuntimeSupported
    } else {
        ConnectionSupportState::Planned
    }
}

fn connection_timeout_inspection(
    connection: &ConnectionDescriptor,
    timeout: Duration,
) -> ConnectionReadinessInspection {
    ConnectionReadinessInspection {
        connection_id: connection.id.clone(),
        provider: connection.provider.clone(),
        default_model: connection.default_model.clone(),
        auth_methods: connection.auth_methods.clone(),
        // Timeout is a launch-time probe result, not proof that auth is missing.
        // Keep auth configured so callers do not open setup just because a
        // remote readiness check was intentionally bounded.
        auth_state: if connection.auth_sources.is_empty() {
            ConnectionAuthState::NotRequired
        } else {
            ConnectionAuthState::Configured
        },
        auth_kind: None,
        auth_source: None,
        auth_tried_sources: Vec::new(),
        support_state: if connection_supported(connection) {
            ConnectionSupportState::RuntimeSupported
        } else {
            ConnectionSupportState::Planned
        },
        readiness_state: ConnectionReadinessState::Unreachable,
        probe_ready: false,
        probe_status: format!(
            "startup readiness timed out after {} ms; full readiness will refresh after launch",
            timeout.as_millis()
        ),
    }
}

fn inspect_web_backend(
    backend: &WebBackendConfig,
    resolver: &CredentialResolver,
) -> Result<WebBackendReadinessInspection> {
    if !backend.enabled {
        return Ok(WebBackendReadinessInspection {
            backend_id: backend.id.clone(),
            provider: backend.kind.provider_name().to_owned(),
            enabled: false,
            auth_state: ConnectionAuthState::Missing,
            auth_kind: None,
            auth_source: None,
            auth_tried_sources: Vec::new(),
            readiness_state: WebBackendReadinessState::Disabled,
            probe_ready: false,
            probe_status: "disabled in config".to_owned(),
        });
    }

    let descriptor = backend.credential_descriptor();
    let credential = match resolver.resolve(&descriptor) {
        Ok(credential) => credential,
        Err(error) => {
            return Ok(WebBackendReadinessInspection {
                backend_id: backend.id.clone(),
                provider: backend.kind.provider_name().to_owned(),
                enabled: true,
                auth_state: ConnectionAuthState::Missing,
                auth_kind: None,
                auth_source: None,
                auth_tried_sources: Vec::new(),
                readiness_state: WebBackendReadinessState::Degraded,
                probe_ready: false,
                probe_status: format!("auth resolution failed: {error}"),
            });
        }
    };

    if let Some(credential) = credential {
        return Ok(WebBackendReadinessInspection {
            backend_id: backend.id.clone(),
            provider: backend.kind.provider_name().to_owned(),
            enabled: true,
            auth_state: ConnectionAuthState::Configured,
            auth_kind: Some(credential.kind.clone()),
            auth_source: Some(credential.source.clone()),
            auth_tried_sources: credential.tried_sources.clone(),
            readiness_state: WebBackendReadinessState::Ready,
            probe_ready: true,
            probe_status: format!("configured via {}", credential_summary(&credential)),
        });
    }

    let status = if backend.auth_sources.is_empty() {
        "no auth sources configured".to_owned()
    } else {
        format!(
            "missing auth; run `belltower web login {}` or set {}",
            backend.id,
            backend.auth_sources.join(", ")
        )
    };
    Ok(WebBackendReadinessInspection {
        backend_id: backend.id.clone(),
        provider: backend.kind.provider_name().to_owned(),
        enabled: true,
        auth_state: ConnectionAuthState::Missing,
        auth_kind: None,
        auth_source: None,
        auth_tried_sources: Vec::new(),
        readiness_state: WebBackendReadinessState::MissingAuth,
        probe_ready: false,
        probe_status: status,
    })
}

#[derive(Clone, Debug)]
struct ReadinessCredential {
    credential: Option<ResolvedCredential>,
    error: Option<String>,
}

async fn resolve_credential_for_readiness(
    resolver: &CredentialResolver,
    connection: &ConnectionDescriptor,
) -> ReadinessCredential {
    match resolver.resolve_fresh(connection).await {
        Ok(credential) => ReadinessCredential {
            credential,
            error: None,
        },
        Err(error) => ReadinessCredential {
            credential: resolver.resolve(connection).ok().flatten(),
            error: Some(error.to_string()),
        },
    }
}

fn credential_resolution_failure_probe(error: &str) -> ConnectionProbe {
    ConnectionProbe {
        state: ConnectionReadinessState::ValidationFailed,
        ready: false,
        status: format!("auth resolution failed: {error}"),
    }
}

fn build_connection_model_inventory(
    connection: &ConnectionDescriptor,
    inspection: ConnectionReadinessInspection,
    discovered_source: Option<String>,
    discovered_models: Vec<String>,
) -> ConnectionModelInventory {
    let discovered_models = curated_connection_models(connection, discovered_models);
    let mut seen = BTreeSet::new();
    let mut models = Vec::new();
    let discovery_available = discovered_source.is_some();

    if discovery_available {
        if discovered_models
            .iter()
            .any(|model| model == &connection.default_model)
        {
            push_model_option(
                &mut models,
                &mut seen,
                connection.default_model.clone(),
                ConnectionModelSource::Default,
            );
        }
        for model in discovered_models {
            push_model_option(
                &mut models,
                &mut seen,
                model,
                ConnectionModelSource::Discovered,
            );
        }
    } else {
        push_model_option(
            &mut models,
            &mut seen,
            connection.default_model.clone(),
            ConnectionModelSource::Default,
        );
        for model in &connection.model_fallbacks {
            push_model_option(
                &mut models,
                &mut seen,
                model.clone(),
                ConnectionModelSource::Fallback,
            );
        }
    }

    ConnectionModelInventory {
        connection_id: inspection.connection_id,
        provider: inspection.provider,
        default_model: inspection.default_model,
        auth_methods: inspection.auth_methods,
        auth_state: inspection.auth_state,
        auth_source: inspection.auth_source,
        support_state: inspection.support_state,
        readiness_state: inspection.readiness_state,
        probe_ready: inspection.probe_ready,
        probe_status: inspection.probe_status,
        discovered_source,
        models,
    }
}

fn push_model_option(
    models: &mut Vec<ConnectionModelOption>,
    seen: &mut BTreeSet<String>,
    model_id: String,
    source: ConnectionModelSource,
) {
    if model_id.trim().is_empty() || !seen.insert(model_id.clone()) {
        return;
    }
    models.push(ConnectionModelOption { model_id, source });
}

fn curated_connection_models(
    connection: &ConnectionDescriptor,
    discovered_models: Vec<String>,
) -> Vec<String> {
    if connection.discoverable_model_selectors.is_empty() {
        return discovered_models
            .into_iter()
            .filter(|model| !model.trim().is_empty())
            .collect();
    }

    let mut curated = Vec::new();
    let mut seen = BTreeSet::new();
    for selector in &connection.discoverable_model_selectors {
        for model in &discovered_models {
            if model.trim().is_empty() || !selector.matches(model) || !seen.insert(model.clone()) {
                continue;
            }
            curated.push(model.clone());
        }
    }
    curated
}

fn model_is_available_for_connection(
    connection: &ConnectionDescriptor,
    discovered_models: &[String],
    model_id: &str,
) -> bool {
    if discovered_models.iter().any(|model| model == model_id) {
        return true;
    }

    connection
        .discoverable_model_selectors
        .iter()
        .filter(|selector| match selector {
            bt_core::ConnectionModelSelector::Exact { value }
            | bt_core::ConnectionModelSelector::Prefix { value } => value == model_id,
        })
        .any(|selector| {
            discovered_models
                .iter()
                .any(|model| selector.matches(model))
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ConnectionProbe {
    state: ConnectionReadinessState,
    ready: bool,
    status: String,
}

async fn probe_connection_readiness(
    config: &BelltowerConfig,
    connection: &ConnectionDescriptor,
    credential: Option<&ResolvedCredential>,
) -> Result<ConnectionProbe> {
    if !connection_supported(connection) {
        return Ok(ConnectionProbe {
            state: ConnectionReadinessState::RuntimeUnsupported,
            ready: false,
            status: "planned; provider not implemented yet".to_owned(),
        });
    }

    if !connection.auth_sources.is_empty() && credential.is_none() {
        return Ok(ConnectionProbe {
            state: ConnectionReadinessState::MissingAuth,
            ready: false,
            status: format!("missing auth; run `belltower login {}`", connection.id),
        });
    }

    if let Some(credential) = credential
        && !credential_supported_by_connection(connection, credential)
    {
        return Ok(ConnectionProbe {
            state: ConnectionReadinessState::UnsupportedAuthMethod,
            ready: false,
            status: format!(
                "configured {}, but `{}` currently supports {}",
                credential_summary(credential),
                connection.id,
                supported_auth_methods_summary(connection)
            ),
        });
    }

    let base_probe = validate_connection(
        connection,
        credential,
        Duration::from_millis(config.models.request_timeout_ms.max(1_000)),
    )
    .await?;

    if connection.id == bt_core::ConnectionId::new("local") {
        return augment_local_probe(config, connection, base_probe).await;
    }

    augment_remote_probe(config, connection, credential, base_probe).await
}

async fn discover_connection_models(
    config: &BelltowerConfig,
    connection: &ConnectionDescriptor,
    credential: Option<&ResolvedCredential>,
) -> Result<(Option<String>, Vec<String>)> {
    match try_discover_connection_models(config, connection, credential).await {
        Ok(discovery) => Ok(discovery),
        Err(_) => Ok((None, Vec::new())),
    }
}

async fn try_discover_connection_models(
    config: &BelltowerConfig,
    connection: &ConnectionDescriptor,
    credential: Option<&ResolvedCredential>,
) -> Result<(Option<String>, Vec<String>)> {
    if !connection_supported(connection) {
        return Ok((None, Vec::new()));
    }

    if connection.id == bt_core::ConnectionId::new("local") {
        let local_models = configured_local_models(config, connection).await?;
        return Ok((local_models.backend_label, local_models.models));
    }

    if !connection.auth_sources.is_empty() && credential.is_none() {
        return Ok((None, Vec::new()));
    }

    if let Some(credential) = credential
        && !credential_supported_by_connection(connection, credential)
    {
        return Ok((None, Vec::new()));
    }

    let provider = provider_for_connection(
        connection,
        credential.map(ResolvedCredential::runtime_credential),
    )?;
    let models = provider.list_models().await?;
    Ok((Some(connection.provider.clone()), models))
}

async fn validate_connection(
    connection: &ConnectionDescriptor,
    credential: Option<&ResolvedCredential>,
    timeout: Duration,
) -> Result<ConnectionProbe> {
    let provider = provider_for_connection(
        connection,
        credential.map(ResolvedCredential::runtime_credential),
    )?;

    let source_suffix = credential
        .map(|credential| format!(" via {}", credential_summary(credential)))
        .unwrap_or_default();

    let status = tokio::time::timeout(timeout, provider.validate()).await;
    let probe = match status {
        Ok(Ok(ConnectionStatus::Healthy)) => ConnectionProbe {
            state: ConnectionReadinessState::Ready,
            ready: true,
            status: format!("healthy{source_suffix}"),
        },
        Ok(Ok(ConnectionStatus::Degraded { reason })) => ConnectionProbe {
            state: ConnectionReadinessState::Degraded,
            ready: false,
            status: format!("degraded: {reason}{source_suffix}"),
        },
        Ok(Ok(ConnectionStatus::Unreachable { reason })) => ConnectionProbe {
            state: ConnectionReadinessState::Unreachable,
            ready: false,
            status: format!("unreachable: {reason}{source_suffix}"),
        },
        Ok(Ok(ConnectionStatus::Unknown)) => ConnectionProbe {
            state: ConnectionReadinessState::UnknownState,
            ready: false,
            status: format!("unknown state{source_suffix}"),
        },
        Ok(Err(error)) => ConnectionProbe {
            state: ConnectionReadinessState::ValidationFailed,
            ready: false,
            status: format!("validation failed: {error}{source_suffix}"),
        },
        Err(_) => ConnectionProbe {
            state: ConnectionReadinessState::Unreachable,
            ready: false,
            status: format!(
                "unreachable: timed out after {} ms{source_suffix}",
                timeout.as_millis()
            ),
        },
    };
    Ok(probe)
}

fn credential_summary(credential: &ResolvedCredential) -> String {
    let kind = match credential.kind {
        CredentialKind::ApiKey => "secret",
        CredentialKind::OAuthToken => "oauth token",
        CredentialKind::JsonDocument => "json credentials",
    };
    if credential.tried_sources.is_empty() {
        format!("{kind} from {}", credential.source)
    } else {
        format!(
            "{kind} from {} (tried: {})",
            credential.source,
            credential.tried_sources.join(" -> ")
        )
    }
}

fn credential_supported_by_connection(
    connection: &ConnectionDescriptor,
    credential: &ResolvedCredential,
) -> bool {
    if connection.auth_methods.is_empty() {
        return true;
    }

    connection
        .auth_methods
        .iter()
        .any(|method| auth_method_matches_credential(&method.kind, &credential.kind))
}

fn auth_method_matches_credential(method: &AuthMethodKind, credential: &CredentialKind) -> bool {
    matches!(
        (method, credential),
        (AuthMethodKind::ApiKey, CredentialKind::ApiKey)
            | (AuthMethodKind::OAuthBrowser, CredentialKind::OAuthToken)
            | (AuthMethodKind::OAuthDeviceCode, CredentialKind::OAuthToken)
            | (AuthMethodKind::JsonDocument, CredentialKind::JsonDocument)
    )
}

fn supported_auth_methods_summary(connection: &ConnectionDescriptor) -> String {
    if connection.auth_methods.is_empty() {
        return "the configured auth path".to_owned();
    }

    connection
        .auth_methods
        .iter()
        .map(|method| method.label.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

async fn augment_local_probe(
    config: &BelltowerConfig,
    connection: &ConnectionDescriptor,
    base_probe: ConnectionProbe,
) -> Result<ConnectionProbe> {
    if !base_probe.ready {
        return Ok(base_probe);
    }

    let local_models = configured_local_models(config, connection).await?;
    let Some(backend_label) = local_models.backend_label else {
        return Ok(base_probe);
    };

    if local_models.models.is_empty() {
        return Ok(ConnectionProbe {
            state: ConnectionReadinessState::NoLocalModelsDetected,
            ready: false,
            status: format!(
                "healthy on {backend_label}, but no models were detected for the configured local backend"
            ),
        });
    }

    if !local_models.models.contains(&connection.default_model) {
        return Ok(ConnectionProbe {
            state: ConnectionReadinessState::ConfiguredModelUnavailable,
            ready: false,
            status: format!(
                "healthy on {backend_label}, but configured model `{}` is not available",
                connection.default_model
            ),
        });
    }

    Ok(ConnectionProbe {
        state: ConnectionReadinessState::Ready,
        ready: true,
        status: format!(
            "healthy on {backend_label} with model `{}`",
            connection.default_model
        ),
    })
}

async fn augment_remote_probe(
    config: &BelltowerConfig,
    connection: &ConnectionDescriptor,
    credential: Option<&ResolvedCredential>,
    base_probe: ConnectionProbe,
) -> Result<ConnectionProbe> {
    if !base_probe.ready {
        return Ok(base_probe);
    }

    let (discovered_source, raw_discovered_models) =
        discover_connection_models(config, connection, credential).await?;
    if raw_discovered_models.is_empty() {
        return Ok(base_probe);
    }

    let discovered_models = curated_connection_models(connection, raw_discovered_models);
    if model_is_available_for_connection(connection, &discovered_models, &connection.default_model)
    {
        return Ok(base_probe);
    }

    let discovered_source = discovered_source.unwrap_or_else(|| connection.provider.clone());
    Ok(ConnectionProbe {
        state: ConnectionReadinessState::ConfiguredModelUnavailable,
        ready: false,
        status: format!(
            "{}, but configured model `{}` is not available on {}",
            base_probe.status, connection.default_model, discovered_source
        ),
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ConfiguredLocalModels {
    backend_label: Option<String>,
    models: Vec<String>,
}

async fn configured_local_models(
    config: &BelltowerConfig,
    connection: &ConnectionDescriptor,
) -> Result<ConfiguredLocalModels> {
    let manager = LocalModelManager::from_config(config)?;
    let backends = manager.backends().await;
    let matching = backends
        .into_iter()
        .find(|backend| local_backend_matches(&backend.base_url, &connection.base_url));

    if let Some(backend) = matching {
        let models = if matches!(backend.status, ModelBackendStatus::Ready) {
            backend
                .available_models
                .into_iter()
                .filter(|model| !model.trim().is_empty())
                .collect()
        } else {
            Vec::new()
        };
        return Ok(ConfiguredLocalModels {
            backend_label: Some(backend.label),
            models,
        });
    }

    Ok(ConfiguredLocalModels::default())
}

fn local_backend_matches(backend_base_url: &Url, connection_base_url: &Url) -> bool {
    normalize_local_base_url(backend_base_url) == normalize_local_base_url(connection_base_url)
}

fn normalize_local_base_url(url: &Url) -> String {
    let mut normalized = url.clone();
    normalized.set_query(None);
    normalized.set_fragment(None);
    let path = normalized.path().trim_end_matches('/').to_owned();
    let path = path.strip_suffix("/v1").unwrap_or(&path).to_owned();
    if path.is_empty() {
        normalized.set_path("/");
    } else {
        normalized.set_path(&path);
    }
    normalized.to_string().trim_end_matches('/').to_owned()
}

pub fn backend_models_detail(backend: &ModelBackendDescriptor) -> String {
    if backend.available_models.is_empty() {
        "models=0".to_owned()
    } else {
        format!("models={}", backend.available_models.join(", "))
    }
}

pub fn format_backend_status(status: &ModelBackendStatus) -> String {
    match status {
        ModelBackendStatus::Ready => "ready".to_owned(),
        ModelBackendStatus::Disabled => "disabled".to_owned(),
        ModelBackendStatus::Unreachable { reason } => format!("unreachable: {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        inspect_connection, inspect_connection_model_inventory, inspect_connection_models,
        inspect_connection_status, inspect_connection_status_with_timeout, inspect_status,
        inspect_web_status, probe_connection_readiness, validate_connection,
    };
    use axum::routing::get;
    use axum::{Json, Router, serve};
    use bt_auth::{CredentialResolver, FileAuthStore, ResolvedCredential};
    use bt_core::{
        AuthMethodKind, BelltowerConfig, ConnectionAuthMethodDescriptor, ConnectionDescriptor,
        ConnectionId, ConnectionModelSelector, ConnectionModelSource, ConnectionReadinessState,
        CredentialKind, CredentialMetadata, Credentials, WebBackendConfig, WebBackendKind,
        WebBackendReadinessState,
    };
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::net::TcpListener;

    fn test_connection(provider: &str) -> ConnectionDescriptor {
        ConnectionDescriptor {
            id: ConnectionId::new(provider),
            provider: provider.to_owned(),
            base_url: "https://example.com/v1".parse().expect("url"),
            default_model: "test-model".to_owned(),
            auth_methods: Vec::new(),
            auth_sources: Vec::new(),
            model_fallbacks: Vec::new(),
            discoverable_model_selectors: Vec::new(),
        }
    }

    fn api_key_auth_method() -> ConnectionAuthMethodDescriptor {
        ConnectionAuthMethodDescriptor {
            id: "api_key".to_owned(),
            kind: AuthMethodKind::ApiKey,
            label: "API key".to_owned(),
            source_hint: None,
            supports_refresh: false,
        }
    }

    fn temp_store() -> (TempDir, FileAuthStore) {
        let temp = TempDir::new().expect("tempdir");
        let store = FileAuthStore::new(temp.path().join("auth.json"));
        (temp, store)
    }

    #[tokio::test]
    async fn inspect_status_marks_missing_auth_with_structured_state() {
        let mut config = BelltowerConfig::from_embedded().expect("config");
        config.defaults.default_connection = ConnectionId::new("missing-openai");
        config.connections = vec![ConnectionDescriptor {
            id: ConnectionId::new("missing-openai"),
            provider: "openai-compatible".to_owned(),
            base_url: "https://api.openai.com/v1".parse().expect("url"),
            default_model: "gpt-5.1".to_owned(),
            auth_methods: Vec::new(),
            auth_sources: vec!["env:BT_TEST_MISSING_OPENAI_KEY".to_owned()],
            model_fallbacks: Vec::new(),
            discoverable_model_selectors: Vec::new(),
        }];
        let inspection = inspect_status(&config).await.expect("inspection");
        let connection = inspection
            .connections
            .iter()
            .find(|connection| connection.connection_id == ConnectionId::new("missing-openai"))
            .expect("synthetic connection");
        assert_eq!(
            connection.readiness_state,
            ConnectionReadinessState::MissingAuth
        );
        assert!(!connection.is_ready());
        assert!(connection.probe_status.contains("missing auth"));
    }

    #[test]
    fn inspect_web_status_reports_missing_backend_auth() {
        let mut config = BelltowerConfig::from_embedded().expect("config");
        config.web.search_backend = Some("test-exa".to_owned());
        config.web.backends = vec![WebBackendConfig {
            id: "test-exa".to_owned(),
            kind: WebBackendKind::Exa,
            enabled: true,
            base_url: "https://api.exa.ai".parse().expect("url"),
            auth_sources: vec!["env:BT_TEST_MISSING_EXA_KEY".to_owned()],
        }];

        let inspection = inspect_web_status(&config).expect("inspection");
        assert_eq!(inspection.search_backend.as_deref(), Some("test-exa"));
        assert_eq!(inspection.backends.len(), 1);
        let backend = &inspection.backends[0];
        assert_eq!(backend.backend_id, "test-exa");
        assert_eq!(
            backend.readiness_state,
            WebBackendReadinessState::MissingAuth
        );
        assert!(!backend.is_ready());
        assert!(backend.probe_status.contains("belltower web login"));
    }

    #[tokio::test]
    async fn inspect_connection_status_only_probes_selected_connection() {
        async fn serve_models(hits: Arc<AtomicUsize>) -> String {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let addr = listener.local_addr().expect("addr");
            let route_hits = hits.clone();
            tokio::spawn(async move {
                serve(
                    listener,
                    Router::new().route(
                        "/v1/models",
                        get(move || {
                            let route_hits = route_hits.clone();
                            async move {
                                route_hits.fetch_add(1, Ordering::SeqCst);
                                Json(json!({ "data": [{ "id": "test-model" }] }))
                            }
                        }),
                    ),
                )
                .await
                .expect("server");
            });
            format!("http://{addr}/v1")
        }

        let selected_hits = Arc::new(AtomicUsize::new(0));
        let skipped_hits = Arc::new(AtomicUsize::new(0));
        let selected_base_url = serve_models(selected_hits.clone()).await;
        let skipped_base_url = serve_models(skipped_hits.clone()).await;

        let mut config = BelltowerConfig::from_embedded().expect("config");
        config.connections = vec![
            ConnectionDescriptor {
                id: ConnectionId::new("selected"),
                provider: "openai-compatible".to_owned(),
                base_url: selected_base_url.parse().expect("url"),
                default_model: "test-model".to_owned(),
                auth_methods: Vec::new(),
                auth_sources: Vec::new(),
                model_fallbacks: Vec::new(),
                discoverable_model_selectors: Vec::new(),
            },
            ConnectionDescriptor {
                id: ConnectionId::new("skipped"),
                provider: "openai-compatible".to_owned(),
                base_url: skipped_base_url.parse().expect("url"),
                default_model: "test-model".to_owned(),
                auth_methods: Vec::new(),
                auth_sources: Vec::new(),
                model_fallbacks: Vec::new(),
                discoverable_model_selectors: Vec::new(),
            },
        ];

        let inspection = inspect_connection_status(&config, &ConnectionId::new("selected"))
            .await
            .expect("inspection");
        assert_eq!(inspection.connection_id, ConnectionId::new("selected"));
        assert!(selected_hits.load(Ordering::SeqCst) > 0);
        assert_eq!(skipped_hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn inspect_connection_status_timeout_returns_structured_unreachable_state() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            serve(
                listener,
                Router::new().route(
                    "/v1/models",
                    get(|| async move {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        Json(json!({ "data": [{ "id": "test-model" }] }))
                    }),
                ),
            )
            .await
            .expect("server");
        });

        let mut config = BelltowerConfig::from_embedded().expect("config");
        config.connections = vec![ConnectionDescriptor {
            id: ConnectionId::new("slow"),
            provider: "openai-compatible".to_owned(),
            base_url: format!("http://{addr}/v1").parse().expect("url"),
            default_model: "test-model".to_owned(),
            auth_methods: Vec::new(),
            auth_sources: Vec::new(),
            model_fallbacks: Vec::new(),
            discoverable_model_selectors: Vec::new(),
        }];

        let inspection = inspect_connection_status_with_timeout(
            &config,
            &ConnectionId::new("slow"),
            Duration::from_millis(1),
        )
        .await
        .expect("inspection");

        assert_eq!(inspection.connection_id, ConnectionId::new("slow"));
        assert_eq!(
            inspection.readiness_state,
            ConnectionReadinessState::Unreachable
        );
        assert!(!inspection.is_ready());
        assert!(
            inspection
                .probe_status
                .contains("startup readiness timed out")
        );
    }

    #[tokio::test]
    async fn inspect_connection_reports_chatgpt_refresh_failure_without_failing_inspection() {
        let (_temp, store) = temp_store();
        store
            .store_credentials_sync(
                "chatgpt",
                Credentials {
                    provider: "chatgpt".to_owned(),
                    kind: CredentialKind::OAuthToken,
                    secret: "not-a-jwt".to_owned(),
                    refresh_secret: Some("refresh-token".to_owned()),
                    metadata: CredentialMetadata {
                        account_id: Some("acct-123".to_owned()),
                        plan_type: Some("plus".to_owned()),
                        workspace_id: None,
                    },
                },
            )
            .expect("store credential");
        let resolver = CredentialResolver::with_store(store);
        let config = BelltowerConfig::from_embedded().expect("config");
        let connection = ConnectionDescriptor {
            id: ConnectionId::new("chatgpt"),
            provider: "openai-chatgpt".to_owned(),
            base_url: "https://chatgpt.com/backend-api".parse().expect("url"),
            default_model: "gpt-5.4-mini".to_owned(),
            auth_methods: Vec::new(),
            auth_sources: vec!["auth_store".to_owned()],
            model_fallbacks: Vec::new(),
            discoverable_model_selectors: Vec::new(),
        };

        let inspection = inspect_connection(&config, &connection, &resolver)
            .await
            .expect("inspection should degrade, not fail");

        assert_eq!(
            inspection.auth_state,
            bt_core::ConnectionAuthState::Configured
        );
        assert_eq!(
            inspection.readiness_state,
            ConnectionReadinessState::ValidationFailed
        );
        assert!(!inspection.is_ready());
        assert!(
            inspection
                .probe_status
                .contains("auth resolution failed: authentication error")
        );
    }

    #[tokio::test]
    async fn inspect_connection_models_carries_structured_readiness_state() {
        let mut config = BelltowerConfig::from_embedded().expect("config");
        config.defaults.default_connection = ConnectionId::new("missing-openai");
        config.connections = vec![ConnectionDescriptor {
            id: ConnectionId::new("missing-openai"),
            provider: "openai-compatible".to_owned(),
            base_url: "https://api.openai.com/v1".parse().expect("url"),
            default_model: "gpt-5.1".to_owned(),
            auth_methods: Vec::new(),
            auth_sources: vec!["env:BT_TEST_MISSING_OPENAI_KEY".to_owned()],
            model_fallbacks: Vec::new(),
            discoverable_model_selectors: Vec::new(),
        }];
        let inventories = inspect_connection_models(&config)
            .await
            .expect("inventories");
        let connection = inventories
            .iter()
            .find(|connection| connection.connection_id == ConnectionId::new("missing-openai"))
            .expect("synthetic inventory");
        assert_eq!(
            connection.readiness_state,
            ConnectionReadinessState::MissingAuth
        );
        assert!(!connection.is_ready());
    }

    #[tokio::test]
    async fn inspect_connection_model_inventory_only_probes_requested_connection() {
        async fn openai_models(
            axum::extract::State(request_count): axum::extract::State<Arc<AtomicUsize>>,
        ) -> Json<serde_json::Value> {
            request_count.fetch_add(1, Ordering::SeqCst);
            Json(json!({ "data": [{ "id": "gpt-5.4-mini" }] }))
        }

        let selected_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let selected_addr = selected_listener.local_addr().expect("addr");
        let selected_count = Arc::new(AtomicUsize::new(0));
        let selected_state = Arc::clone(&selected_count);
        tokio::spawn(async move {
            serve(
                selected_listener,
                Router::new()
                    .route("/v1/models", get(openai_models))
                    .with_state(selected_state),
            )
            .await
            .expect("server");
        });

        let other_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let other_addr = other_listener.local_addr().expect("addr");
        let other_count = Arc::new(AtomicUsize::new(0));
        let other_state = Arc::clone(&other_count);
        tokio::spawn(async move {
            serve(
                other_listener,
                Router::new()
                    .route("/v1/models", get(openai_models))
                    .with_state(other_state),
            )
            .await
            .expect("server");
        });

        let mut config = BelltowerConfig::from_embedded().expect("config");
        config.connections = vec![
            ConnectionDescriptor {
                id: ConnectionId::new("selected"),
                provider: "openai-compatible".to_owned(),
                base_url: format!("http://{selected_addr}/v1").parse().expect("url"),
                default_model: "gpt-5.4-mini".to_owned(),
                auth_methods: Vec::new(),
                auth_sources: Vec::new(),
                model_fallbacks: Vec::new(),
                discoverable_model_selectors: Vec::new(),
            },
            ConnectionDescriptor {
                id: ConnectionId::new("other"),
                provider: "openai-compatible".to_owned(),
                base_url: format!("http://{other_addr}/v1").parse().expect("url"),
                default_model: "gpt-5.4-mini".to_owned(),
                auth_methods: Vec::new(),
                auth_sources: Vec::new(),
                model_fallbacks: Vec::new(),
                discoverable_model_selectors: Vec::new(),
            },
        ];

        let inventory = inspect_connection_model_inventory(&config, &ConnectionId::new("selected"))
            .await
            .expect("inventory");

        assert_eq!(inventory.connection_id, ConnectionId::new("selected"));
        assert_eq!(selected_count.load(Ordering::SeqCst), 1);
        assert_eq!(other_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn probe_marks_unsupported_auth_method_when_runtime_cannot_use_configured_credential() {
        let config = BelltowerConfig::from_embedded().expect("config");
        let mut connection = test_connection("openai");
        connection.provider = "openai-compatible".to_owned();
        connection.auth_methods = vec![api_key_auth_method()];
        connection.auth_sources = vec!["auth_store".to_owned()];
        let credential = ResolvedCredential {
            provider: "openai".to_owned(),
            kind: CredentialKind::OAuthToken,
            source: "auth store".to_owned(),
            tried_sources: Vec::new(),
            secret: "oauth-access".to_owned(),
            refresh_secret: Some("oauth-refresh".to_owned()),
            metadata: bt_core::CredentialMetadata {
                account_id: Some("acct-123".to_owned()),
                plan_type: Some("pro".to_owned()),
                workspace_id: Some("ws-main".to_owned()),
            },
        };

        let probe = probe_connection_readiness(&config, &connection, Some(&credential))
            .await
            .expect("probe");
        assert_eq!(probe.state, ConnectionReadinessState::UnsupportedAuthMethod);
        assert!(!probe.ready);
        assert!(probe.status.contains("oauth token"));
        assert!(probe.status.contains("API key"));
    }

    #[tokio::test]
    async fn validate_connection_maps_degraded_to_structured_state() {
        async fn models() -> axum::http::StatusCode {
            axum::http::StatusCode::UNAUTHORIZED
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            serve(listener, Router::new().route("/v1/models", get(models)))
                .await
                .expect("server");
        });

        let connection = ConnectionDescriptor {
            id: ConnectionId::new("anthropic"),
            provider: "anthropic".to_owned(),
            base_url: format!("http://{addr}/").parse().expect("url"),
            default_model: "claude-sonnet".to_owned(),
            auth_methods: Vec::new(),
            auth_sources: vec!["env:ANTHROPIC_API_KEY".to_owned()],
            model_fallbacks: Vec::new(),
            discoverable_model_selectors: Vec::new(),
        };
        let credential = ResolvedCredential {
            provider: "anthropic".to_owned(),
            kind: CredentialKind::ApiKey,
            source: "env:ANTHROPIC_API_KEY".to_owned(),
            tried_sources: Vec::new(),
            secret: "sk-ant-test".to_owned(),
            refresh_secret: None,
            metadata: bt_core::CredentialMetadata::default(),
        };

        let probe = validate_connection(&connection, Some(&credential), Duration::from_secs(1))
            .await
            .expect("probe");
        assert_eq!(probe.state, ConnectionReadinessState::Degraded);
        assert!(!probe.ready);
        assert!(probe.status.contains("401"));
    }

    #[tokio::test]
    async fn validate_connection_reports_tried_auth_backends_in_status() {
        async fn models() -> Json<serde_json::Value> {
            Json(json!({ "data": [] }))
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            serve(listener, Router::new().route("/v1/models", get(models)))
                .await
                .expect("server");
        });

        let connection = ConnectionDescriptor {
            id: ConnectionId::new("openai"),
            provider: "openai-compatible".to_owned(),
            base_url: format!("http://{addr}/v1").parse().expect("url"),
            default_model: "o4-mini".to_owned(),
            auth_methods: Vec::new(),
            auth_sources: vec!["auth_store".to_owned()],
            model_fallbacks: Vec::new(),
            discoverable_model_selectors: Vec::new(),
        };
        let credential = ResolvedCredential {
            provider: "openai".to_owned(),
            kind: CredentialKind::ApiKey,
            source: "keychain (service belltower, account openai)".to_owned(),
            tried_sources: vec![
                "ephemeral environment".to_owned(),
                "keychain (service belltower)".to_owned(),
            ],
            secret: "sk-test".to_owned(),
            refresh_secret: None,
            metadata: bt_core::CredentialMetadata::default(),
        };

        let probe = validate_connection(&connection, Some(&credential), Duration::from_secs(1))
            .await
            .expect("probe");
        assert!(probe.ready);
        assert!(
            probe
                .status
                .contains("tried: ephemeral environment -> keychain (service belltower)")
        );
    }

    #[tokio::test]
    async fn local_probe_marks_configured_model_unavailable_with_structured_state() {
        async fn openai_models() -> Json<serde_json::Value> {
            Json(json!({ "data": [{ "id": "qwen3.5:latest" }] }))
        }

        async fn ollama_models() -> Json<serde_json::Value> {
            Json(json!({ "models": [{ "name": "qwen3.5:latest" }] }))
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            serve(
                listener,
                Router::new()
                    .route("/v1/models", get(openai_models))
                    .route("/api/tags", get(ollama_models)),
            )
            .await
            .expect("server");
        });

        let mut config = BelltowerConfig::from_embedded().expect("config");
        config.models.backends[0].base_url = format!("http://{addr}").parse().expect("url");
        config.models.backends[1].enabled = false;
        config.models.backends[2].enabled = false;
        config.connections = vec![ConnectionDescriptor {
            id: ConnectionId::new("local"),
            provider: "openai-compatible".to_owned(),
            base_url: format!("http://{addr}/v1").parse().expect("url"),
            default_model: "qwen3:latest".to_owned(),
            auth_methods: Vec::new(),
            auth_sources: Vec::new(),
            model_fallbacks: Vec::new(),
            discoverable_model_selectors: Vec::new(),
        }];

        let inspection = inspect_status(&config).await.expect("inspection");
        let local = inspection
            .connections
            .iter()
            .find(|connection| connection.connection_id == ConnectionId::new("local"))
            .expect("local connection");
        assert_eq!(
            local.readiness_state,
            ConnectionReadinessState::ConfiguredModelUnavailable
        );
        assert!(!local.is_ready());
        assert!(
            local
                .probe_status
                .contains("configured model `qwen3:latest` is not available")
        );
    }

    #[tokio::test]
    async fn remote_probe_marks_configured_model_unavailable_when_discovery_excludes_default() {
        async fn openai_models() -> Json<serde_json::Value> {
            Json(json!({ "data": [{ "id": "gpt-4.1-mini" }] }))
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            serve(
                listener,
                Router::new().route("/v1/models", get(openai_models)),
            )
            .await
            .expect("server");
        });

        let mut config = BelltowerConfig::from_embedded().expect("config");
        config.connections = vec![ConnectionDescriptor {
            id: ConnectionId::new("remote-openai"),
            provider: "openai-compatible".to_owned(),
            base_url: format!("http://{addr}/v1").parse().expect("url"),
            default_model: "gpt-5.1".to_owned(),
            auth_methods: Vec::new(),
            auth_sources: Vec::new(),
            model_fallbacks: Vec::new(),
            discoverable_model_selectors: Vec::new(),
        }];

        let inspection = inspect_status(&config).await.expect("inspection");
        let remote = inspection
            .connections
            .iter()
            .find(|connection| connection.connection_id == ConnectionId::new("remote-openai"))
            .expect("remote connection");
        assert_eq!(
            remote.readiness_state,
            ConnectionReadinessState::ConfiguredModelUnavailable
        );
        assert!(!remote.is_ready());
        assert!(
            remote
                .probe_status
                .contains("configured model `gpt-5.1` is not available")
        );
    }

    #[tokio::test]
    async fn remote_probe_keeps_ready_state_when_model_inventory_is_empty() {
        async fn openai_models() -> Json<serde_json::Value> {
            Json(json!({ "data": [] }))
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            serve(
                listener,
                Router::new().route("/v1/models", get(openai_models)),
            )
            .await
            .expect("server");
        });

        let mut config = BelltowerConfig::from_embedded().expect("config");
        config.connections = vec![ConnectionDescriptor {
            id: ConnectionId::new("remote-openai"),
            provider: "openai-compatible".to_owned(),
            base_url: format!("http://{addr}/v1").parse().expect("url"),
            default_model: "gpt-5.1".to_owned(),
            auth_methods: Vec::new(),
            auth_sources: Vec::new(),
            model_fallbacks: Vec::new(),
            discoverable_model_selectors: Vec::new(),
        }];

        let inspection = inspect_status(&config).await.expect("inspection");
        let remote = inspection
            .connections
            .iter()
            .find(|connection| connection.connection_id == ConnectionId::new("remote-openai"))
            .expect("remote connection");
        assert_eq!(remote.readiness_state, ConnectionReadinessState::Ready);
        assert!(remote.is_ready());
        assert!(remote.probe_status.starts_with("healthy"));
    }

    #[tokio::test]
    async fn inspect_connection_models_marks_remote_mismatch_with_structured_state() {
        async fn openai_models() -> Json<serde_json::Value> {
            Json(json!({ "data": [{ "id": "gpt-4.1-mini" }] }))
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            serve(
                listener,
                Router::new().route("/v1/models", get(openai_models)),
            )
            .await
            .expect("server");
        });

        let mut config = BelltowerConfig::from_embedded().expect("config");
        config.connections = vec![ConnectionDescriptor {
            id: ConnectionId::new("remote-openai"),
            provider: "openai-compatible".to_owned(),
            base_url: format!("http://{addr}/v1").parse().expect("url"),
            default_model: "gpt-5.1".to_owned(),
            auth_methods: Vec::new(),
            auth_sources: Vec::new(),
            model_fallbacks: Vec::new(),
            discoverable_model_selectors: Vec::new(),
        }];

        let inventories = inspect_connection_models(&config)
            .await
            .expect("inventories");
        let remote = inventories
            .iter()
            .find(|connection| connection.connection_id == ConnectionId::new("remote-openai"))
            .expect("remote inventory");
        assert_eq!(
            remote.readiness_state,
            ConnectionReadinessState::ConfiguredModelUnavailable
        );
        assert!(!remote.is_ready());
        assert_eq!(
            remote.discovered_source.as_deref(),
            Some("openai-compatible")
        );
        assert!(
            remote
                .models
                .iter()
                .any(|model| model.model_id == "gpt-4.1-mini")
        );
    }

    #[tokio::test]
    async fn inspect_connection_models_treats_remote_discovery_failure_as_unavailable() {
        async fn openai_models() -> (axum::http::StatusCode, Json<serde_json::Value>) {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "temporary discovery failure" })),
            )
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            serve(
                listener,
                Router::new().route("/v1/models", get(openai_models)),
            )
            .await
            .expect("server");
        });

        let mut config = BelltowerConfig::from_embedded().expect("config");
        config.connections = vec![ConnectionDescriptor {
            id: ConnectionId::new("remote-openai"),
            provider: "openai-compatible".to_owned(),
            base_url: format!("http://{addr}/v1").parse().expect("url"),
            default_model: "gpt-5.4".to_owned(),
            auth_methods: Vec::new(),
            auth_sources: Vec::new(),
            model_fallbacks: vec!["gpt-5.4-mini".to_owned()],
            discoverable_model_selectors: Vec::new(),
        }];

        let inventories = inspect_connection_models(&config)
            .await
            .expect("inventories");
        let remote = inventories
            .iter()
            .find(|connection| connection.connection_id == ConnectionId::new("remote-openai"))
            .expect("remote inventory");
        assert_eq!(remote.discovered_source, None);
        assert_eq!(
            remote.readiness_state,
            ConnectionReadinessState::ValidationFailed
        );
        assert!(
            remote.models.iter().any(|model| model.model_id == "gpt-5.4"
                && model.source == ConnectionModelSource::Default)
        );
        assert!(
            remote
                .models
                .iter()
                .any(|model| model.model_id == "gpt-5.4-mini"
                    && model.source == ConnectionModelSource::Fallback)
        );
    }

    #[tokio::test]
    async fn inspect_connection_models_filters_openai_inventory_to_curated_models() {
        async fn openai_models() -> Json<serde_json::Value> {
            Json(json!({
                "data": [
                    { "id": "o4-mini" },
                    { "id": "gpt-5.3-codex" },
                    { "id": "gpt-5.4-mini" },
                    { "id": "gpt-5.4" },
                    { "id": "gpt-4.1" }
                ]
            }))
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            serve(
                listener,
                Router::new().route("/v1/models", get(openai_models)),
            )
            .await
            .expect("server");
        });

        let mut config = BelltowerConfig::from_embedded().expect("config");
        config.connections = vec![ConnectionDescriptor {
            id: ConnectionId::new("openai"),
            provider: "openai-compatible".to_owned(),
            base_url: format!("http://{addr}/v1").parse().expect("url"),
            default_model: "gpt-5.4".to_owned(),
            auth_methods: Vec::new(),
            auth_sources: Vec::new(),
            model_fallbacks: Vec::new(),
            discoverable_model_selectors: vec![
                ConnectionModelSelector::Exact {
                    value: "gpt-5.4".to_owned(),
                },
                ConnectionModelSelector::Exact {
                    value: "gpt-5.2-codex".to_owned(),
                },
                ConnectionModelSelector::Exact {
                    value: "gpt-5.1-codex-max".to_owned(),
                },
                ConnectionModelSelector::Exact {
                    value: "gpt-5.4-mini".to_owned(),
                },
                ConnectionModelSelector::Exact {
                    value: "gpt-5.3-codex".to_owned(),
                },
                ConnectionModelSelector::Exact {
                    value: "gpt-5.3-codex-spark".to_owned(),
                },
                ConnectionModelSelector::Exact {
                    value: "gpt-5.2".to_owned(),
                },
                ConnectionModelSelector::Exact {
                    value: "gpt-5.1-codex-mini".to_owned(),
                },
            ],
        }];

        let inventories = inspect_connection_models(&config)
            .await
            .expect("inventories");
        let openai = inventories
            .iter()
            .find(|connection| connection.connection_id == ConnectionId::new("openai"))
            .expect("openai inventory");

        assert_eq!(
            openai
                .models
                .iter()
                .map(|model| (model.model_id.clone(), model.source.clone()))
                .collect::<Vec<_>>(),
            vec![
                ("gpt-5.4".to_owned(), ConnectionModelSource::Default),
                ("gpt-5.4-mini".to_owned(), ConnectionModelSource::Discovered),
                (
                    "gpt-5.3-codex".to_owned(),
                    ConnectionModelSource::Discovered
                ),
            ]
        );
    }

    #[tokio::test]
    async fn inspect_connection_models_filters_anthropic_inventory_to_curated_models() {
        async fn anthropic_models() -> Json<serde_json::Value> {
            Json(json!({
                "data": [
                    { "id": "claude-opus-4-6-20260410" },
                    { "id": "claude-sonnet-4-6-20260410" },
                    { "id": "claude-legacy-experimental" },
                    { "id": "claude-haiku-4-5-20260410" },
                    { "id": "claude-opus-3" }
                ]
            }))
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            serve(
                listener,
                Router::new().route("/v1/models", get(anthropic_models)),
            )
            .await
            .expect("server");
        });

        let mut config = BelltowerConfig::from_embedded().expect("config");
        config.connections = vec![ConnectionDescriptor {
            id: ConnectionId::new("anthropic"),
            provider: "anthropic".to_owned(),
            base_url: format!("http://{addr}/").parse().expect("url"),
            default_model: "claude-opus-4-7".to_owned(),
            auth_methods: Vec::new(),
            auth_sources: Vec::new(),
            model_fallbacks: Vec::new(),
            discoverable_model_selectors: vec![
                ConnectionModelSelector::Prefix {
                    value: "claude-opus-4-7".to_owned(),
                },
                ConnectionModelSelector::Prefix {
                    value: "claude-sonnet-4-6".to_owned(),
                },
                ConnectionModelSelector::Prefix {
                    value: "claude-haiku-4-5".to_owned(),
                },
                ConnectionModelSelector::Prefix {
                    value: "claude-opus-4-6".to_owned(),
                },
                ConnectionModelSelector::Prefix {
                    value: "claude-opus-3".to_owned(),
                },
                ConnectionModelSelector::Prefix {
                    value: "claude-sonnet-4-5".to_owned(),
                },
            ],
        }];

        let inventories = inspect_connection_models(&config)
            .await
            .expect("inventories");
        let anthropic = inventories
            .iter()
            .find(|connection| connection.connection_id == ConnectionId::new("anthropic"))
            .expect("anthropic inventory");

        assert_eq!(
            anthropic
                .models
                .iter()
                .map(|model| model.model_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "claude-sonnet-4-6-20260410",
                "claude-haiku-4-5-20260410",
                "claude-opus-4-6-20260410",
                "claude-opus-3",
            ]
        );
        assert!(
            anthropic
                .models
                .iter()
                .all(|model| model.source == ConnectionModelSource::Discovered)
        );
    }

    #[tokio::test]
    async fn remote_probe_accepts_selector_alias_when_discovery_matches_family() {
        async fn anthropic_models() -> Json<serde_json::Value> {
            Json(json!({
                "data": [
                    { "id": "claude-sonnet-4-6-20260410" }
                ]
            }))
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            serve(
                listener,
                Router::new().route("/v1/models", get(anthropic_models)),
            )
            .await
            .expect("server");
        });

        let mut config = BelltowerConfig::from_embedded().expect("config");
        config.connections = vec![ConnectionDescriptor {
            id: ConnectionId::new("anthropic"),
            provider: "anthropic".to_owned(),
            base_url: format!("http://{addr}/").parse().expect("url"),
            default_model: "claude-sonnet-4-6".to_owned(),
            auth_methods: Vec::new(),
            auth_sources: Vec::new(),
            model_fallbacks: Vec::new(),
            discoverable_model_selectors: vec![ConnectionModelSelector::Prefix {
                value: "claude-sonnet-4-6".to_owned(),
            }],
        }];

        let inspection = inspect_status(&config).await.expect("inspection");
        let anthropic = inspection
            .connections
            .iter()
            .find(|connection| connection.connection_id == ConnectionId::new("anthropic"))
            .expect("anthropic connection");
        assert_eq!(anthropic.readiness_state, ConnectionReadinessState::Ready);
        assert!(anthropic.is_ready());
    }

    #[tokio::test]
    async fn unsupported_provider_marks_runtime_unsupported_state() {
        let config = BelltowerConfig::from_embedded().expect("config");
        let connection = test_connection("google-vertex");
        let probe = probe_connection_readiness(&config, &connection, None)
            .await
            .expect("probe");
        assert_eq!(probe.state, ConnectionReadinessState::RuntimeUnsupported);
        assert!(!probe.ready);
    }
}
