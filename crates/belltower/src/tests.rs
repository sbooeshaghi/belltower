//! Tests for launcher CLI setup, readiness, helper discovery, and auth-token compatibility.

use super::launcher::{
    WorkspaceHelperMode, server_auth_compatible_with_token_path, server_auth_token_candidate_paths,
    server_protocol_ready_with_token_paths, sibling_binary_is_fresh_for, workspace_helper_mode_for,
};
use super::{
    ConnectionChoice, ConnectionPromptMode, auth_methods_support_device_code_login,
    configured_local_models, connection_needs_auth_setup, connection_needs_launch_setup,
    connection_prompt_allows, connection_support_label, connection_supported,
    connection_supports_api_key_login, doctor_connection_detail, headless_display_name,
    normalize_local_base_url, render_headless_assistant_output, resolve_secret_input,
    should_fallback_to_local_connection,
};
use axum::routing::get;
use axum::{Json, Router, serve};
use bt_auth::{CredentialInput, ResolvedCredential};
use bt_core::{
    AuthMethodKind, BelltowerConfig, ConnectionAuthMethodDescriptor, ConnectionDescriptor,
    ConnectionId, ConnectionReadinessInspection, ConnectionSupportState, CredentialKind, Message,
    Result, Role,
};
use bt_providers::provider_for_connection;
use serde_json::json;
use std::fs;
use std::path::Path;
use std::time::Duration;
use tempfile::NamedTempFile;
use tokio::net::TcpListener;
use url::Url;

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
struct ConnectionProbe {
    ready: bool,
    status: String,
}

#[cfg(test)]
async fn validate_connection(
    connection: &ConnectionDescriptor,
    credential: Option<&bt_auth::ResolvedCredential>,
    timeout: Duration,
) -> Result<ConnectionProbe> {
    let provider = provider_for_connection(
        connection,
        credential.map(bt_auth::ResolvedCredential::runtime_credential),
    )?;

    let source_suffix = credential
        .map(|credential| format!(" via {}", credential_summary(credential)))
        .unwrap_or_default();

    let status = tokio::time::timeout(timeout, provider.validate()).await;
    let probe = match status {
        Ok(Ok(bt_core::ConnectionStatus::Healthy)) => ConnectionProbe {
            ready: true,
            status: format!("healthy{source_suffix}"),
        },
        Ok(Ok(bt_core::ConnectionStatus::Degraded { reason })) => ConnectionProbe {
            ready: false,
            status: format!("degraded: {reason}{source_suffix}"),
        },
        Ok(Ok(bt_core::ConnectionStatus::Unreachable { reason })) => ConnectionProbe {
            ready: false,
            status: format!("unreachable: {reason}{source_suffix}"),
        },
        Ok(Ok(bt_core::ConnectionStatus::Unknown)) => ConnectionProbe {
            ready: false,
            status: format!("unknown state{source_suffix}"),
        },
        Ok(Err(error)) => ConnectionProbe {
            ready: false,
            status: format!("validation failed: {error}{source_suffix}"),
        },
        Err(_) => ConnectionProbe {
            ready: false,
            status: format!(
                "unreachable: timed out after {} ms{source_suffix}",
                timeout.as_millis()
            ),
        },
    };
    Ok(probe)
}

#[cfg(test)]
fn credential_summary(credential: &bt_auth::ResolvedCredential) -> String {
    let kind = match credential.kind {
        bt_core::CredentialKind::ApiKey => "secret",
        bt_core::CredentialKind::OAuthToken => "oauth token",
        bt_core::CredentialKind::JsonDocument => "json credentials",
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

#[cfg(test)]
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
            ready: false,
            status: format!(
                "healthy on {backend_label}, but no models were detected for the configured local backend"
            ),
        });
    }

    if !local_models.models.contains(&connection.default_model) {
        return Ok(ConnectionProbe {
            ready: false,
            status: format!(
                "healthy on {backend_label}, but configured model `{}` is not available",
                connection.default_model
            ),
        });
    }

    Ok(ConnectionProbe {
        ready: true,
        status: format!(
            "healthy on {backend_label} with model `{}`",
            connection.default_model
        ),
    })
}

fn test_connection(provider: &str) -> ConnectionDescriptor {
    ConnectionDescriptor {
        id: ConnectionId::new(provider),
        provider: provider.to_owned(),
        base_url: Url::parse("https://example.com/v1").expect("url"),
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

#[test]
fn resolve_secret_input_prefers_literal_key() {
    let resolved = resolve_secret_input(
        Some("sk-test".to_owned()),
        Some("HOME".to_owned()),
        Some("printf test".to_owned()),
    )
    .expect("resolve");
    assert_eq!(
        resolved,
        Some(CredentialInput::Literal("sk-test".to_owned()))
    );
}

#[test]
fn resolve_secret_input_supports_env_references() {
    let resolved = resolve_secret_input(None, Some("HOME".to_owned()), None).expect("resolve");
    assert_eq!(
        resolved,
        Some(CredentialInput::EnvReference("HOME".to_owned()))
    );
}

#[test]
fn resolve_secret_input_supports_command_references() {
    let resolved =
        resolve_secret_input(None, None, Some("printf sk-test".to_owned())).expect("resolve");
    assert_eq!(
        resolved,
        Some(CredentialInput::CommandReference(
            "printf sk-test".to_owned()
        ))
    );
}

#[test]
fn headless_display_name_compacts_and_bounds_prompt() {
    let prompt = "  summarize\n\nthis     project  ";
    let display = headless_display_name(prompt);
    assert_eq!(display, "summarize this project");

    let long_display = headless_display_name(&"word ".repeat(40));
    assert!(long_display.chars().count() <= 80);
    assert!(long_display.ends_with("..."));
}

#[test]
fn headless_output_uses_latest_assistant_text() {
    let messages = vec![
        Message::text(Role::User, "hello"),
        Message::text(Role::Assistant, "first"),
        Message::text(Role::User, "again"),
        Message::text(Role::Assistant, "second"),
    ];
    assert_eq!(render_headless_assistant_output(&messages), "second");
}

#[test]
fn runtime_support_only_marks_implemented_providers_supported() {
    assert!(connection_supported(&test_connection("openai-compatible")));
    assert!(connection_supported(&test_connection("anthropic")));
    assert!(!connection_supported(&test_connection("google-vertex")));
}

#[test]
fn support_labels_match_runtime_state() {
    assert_eq!(
        connection_support_label(&test_connection("anthropic")),
        "runtime-supported"
    );
    assert_eq!(
        connection_support_label(&test_connection("google-vertex")),
        "planned"
    );
}

#[test]
fn auth_setup_detection_only_triggers_for_missing_auth_states() {
    let missing = ConnectionReadinessInspection {
        connection_id: ConnectionId::new("openai"),
        provider: "openai-compatible".to_owned(),
        default_model: "o4-mini".to_owned(),
        auth_methods: Vec::new(),
        auth_state: bt_core::ConnectionAuthState::Missing,
        auth_kind: None,
        auth_source: None,
        auth_tried_sources: Vec::new(),
        support_state: ConnectionSupportState::RuntimeSupported,
        readiness_state: bt_core::ConnectionReadinessState::MissingAuth,
        probe_ready: false,
        probe_status: "missing credentials".to_owned(),
    };
    let unavailable = ConnectionReadinessInspection {
        auth_state: bt_core::ConnectionAuthState::Configured,
        readiness_state: bt_core::ConnectionReadinessState::ConfiguredModelUnavailable,
        probe_status: "configured model unavailable".to_owned(),
        ..missing.clone()
    };

    assert!(connection_needs_auth_setup(&missing));
    assert!(!connection_needs_auth_setup(&unavailable));
}

#[test]
fn implicit_remote_default_can_fallback_to_local_when_not_ready() {
    let readiness = ConnectionReadinessInspection {
        connection_id: ConnectionId::new("chatgpt"),
        provider: "openai-chatgpt".to_owned(),
        default_model: "gpt-5.4-mini".to_owned(),
        auth_methods: Vec::new(),
        auth_state: bt_core::ConnectionAuthState::Configured,
        auth_kind: Some(CredentialKind::OAuthToken),
        auth_source: Some("auth store".to_owned()),
        auth_tried_sources: Vec::new(),
        support_state: ConnectionSupportState::RuntimeSupported,
        readiness_state: bt_core::ConnectionReadinessState::ValidationFailed,
        probe_ready: false,
        probe_status: "auth resolution failed".to_owned(),
    };

    assert!(should_fallback_to_local_connection(
        false,
        &ConnectionId::new("chatgpt"),
        &readiness
    ));
    assert!(!should_fallback_to_local_connection(
        true,
        &ConnectionId::new("chatgpt"),
        &readiness
    ));
    assert!(!should_fallback_to_local_connection(
        false,
        &ConnectionId::new("local"),
        &readiness
    ));
}

#[test]
fn implicit_remote_missing_auth_remains_local_fallback_eligible() {
    let readiness = ConnectionReadinessInspection {
        connection_id: ConnectionId::new("chatgpt"),
        provider: "openai-chatgpt".to_owned(),
        default_model: "gpt-5.4-mini".to_owned(),
        auth_methods: Vec::new(),
        auth_state: bt_core::ConnectionAuthState::Missing,
        auth_kind: None,
        auth_source: None,
        auth_tried_sources: Vec::new(),
        support_state: ConnectionSupportState::RuntimeSupported,
        readiness_state: bt_core::ConnectionReadinessState::MissingAuth,
        probe_ready: false,
        probe_status: "missing auth".to_owned(),
    };

    assert!(should_fallback_to_local_connection(
        false,
        &ConnectionId::new("chatgpt"),
        &readiness
    ));
    assert!(connection_needs_launch_setup(
        &ConnectionId::new("chatgpt"),
        &readiness
    ));
}

#[test]
fn launch_readiness_timeout_does_not_force_setup() {
    let readiness = ConnectionReadinessInspection {
        connection_id: ConnectionId::new("chatgpt"),
        provider: "openai-chatgpt".to_owned(),
        default_model: "gpt-5.4-mini".to_owned(),
        auth_methods: Vec::new(),
        auth_state: bt_core::ConnectionAuthState::Configured,
        auth_kind: None,
        auth_source: None,
        auth_tried_sources: Vec::new(),
        support_state: ConnectionSupportState::RuntimeSupported,
        readiness_state: bt_core::ConnectionReadinessState::Unreachable,
        probe_ready: false,
        probe_status: "startup readiness timed out after 750 ms".to_owned(),
    };

    assert!(should_fallback_to_local_connection(
        false,
        &ConnectionId::new("chatgpt"),
        &readiness
    ));
    assert!(!connection_needs_launch_setup(
        &ConnectionId::new("chatgpt"),
        &readiness
    ));
}

#[test]
fn doctor_connection_detail_reports_resolved_backend_and_tried_order() {
    let connection = ConnectionReadinessInspection {
        connection_id: ConnectionId::new("openai"),
        provider: "openai-compatible".to_owned(),
        default_model: "o4-mini".to_owned(),
        auth_methods: Vec::new(),
        auth_state: bt_core::ConnectionAuthState::Configured,
        auth_kind: Some(CredentialKind::ApiKey),
        auth_source: Some("keychain (service belltower, account openai)".to_owned()),
        auth_tried_sources: vec![
            "ephemeral environment".to_owned(),
            "keychain (service belltower)".to_owned(),
        ],
        support_state: ConnectionSupportState::RuntimeSupported,
        readiness_state: bt_core::ConnectionReadinessState::Ready,
        probe_ready: true,
        probe_status: "healthy via secret from keychain (service belltower, account openai)"
            .to_owned(),
    };

    let detail = doctor_connection_detail(&connection);
    assert!(
        detail.contains("credential resolved from keychain (service belltower, account openai)")
    );
    assert!(detail.contains("tried: ephemeral environment -> keychain (service belltower)"));
}

#[test]
fn runnable_prompt_excludes_unimplemented_providers() {
    let supported = ConnectionChoice {
        id: ConnectionId::new("openai"),
        provider: "openai-compatible".to_owned(),
        default_model: "o4-mini".to_owned(),
        ready: true,
        supported: true,
        auth_required: true,
        api_key_login_supported: true,
        device_code_login_supported: false,
        recommended: true,
        status: "ready".to_owned(),
    };
    let planned = ConnectionChoice {
        id: ConnectionId::new("vertex"),
        provider: "google-vertex".to_owned(),
        default_model: "gemini-2.5-pro".to_owned(),
        ready: false,
        supported: false,
        auth_required: true,
        api_key_login_supported: false,
        device_code_login_supported: false,
        recommended: false,
        status: "planned".to_owned(),
    };

    assert!(connection_prompt_allows(
        ConnectionPromptMode::Runnable,
        &supported
    ));
    assert!(!connection_prompt_allows(
        ConnectionPromptMode::Runnable,
        &planned
    ));
}

#[test]
fn login_prompt_excludes_local_connections() {
    let local = ConnectionChoice {
        id: ConnectionId::new("local"),
        provider: "openai-compatible".to_owned(),
        default_model: "qwen3:latest".to_owned(),
        ready: true,
        supported: true,
        auth_required: false,
        api_key_login_supported: false,
        device_code_login_supported: false,
        recommended: true,
        status: "local backend ready".to_owned(),
    };
    assert!(!connection_prompt_allows(
        ConnectionPromptMode::Loginable,
        &local
    ));
}

#[test]
fn login_prompt_allows_planned_device_code_connections() {
    let chatgpt = ConnectionChoice {
        id: ConnectionId::new("chatgpt"),
        provider: "openai-chatgpt".to_owned(),
        default_model: "gpt-5.4-mini".to_owned(),
        ready: false,
        supported: false,
        auth_required: true,
        api_key_login_supported: false,
        device_code_login_supported: true,
        recommended: false,
        status: "planned".to_owned(),
    };
    assert!(connection_prompt_allows(
        ConnectionPromptMode::Loginable,
        &chatgpt
    ));
}

#[test]
fn api_key_login_support_follows_declared_auth_methods() {
    let mut api_key = test_connection("openai");
    api_key.auth_methods = vec![api_key_auth_method()];
    api_key.auth_sources = vec!["env:OPENAI_API_KEY".to_owned()];
    assert!(connection_supports_api_key_login(&api_key));

    let mut oauth = test_connection("future-oauth");
    oauth.auth_methods = vec![ConnectionAuthMethodDescriptor {
        id: "oauth_browser".to_owned(),
        kind: AuthMethodKind::OAuthBrowser,
        label: "Browser OAuth".to_owned(),
        source_hint: None,
        supports_refresh: true,
    }];
    oauth.auth_sources = vec!["auth_store".to_owned()];
    assert!(!connection_supports_api_key_login(&oauth));
}

#[test]
fn device_code_login_support_follows_declared_auth_methods() {
    let oauth_methods = vec![ConnectionAuthMethodDescriptor {
        id: "chatgpt_device_code".to_owned(),
        kind: AuthMethodKind::OAuthDeviceCode,
        label: "ChatGPT device code".to_owned(),
        source_hint: Some("auth_store".to_owned()),
        supports_refresh: true,
    }];
    assert!(auth_methods_support_device_code_login(&oauth_methods));
    assert!(!auth_methods_support_device_code_login(&[
        ConnectionAuthMethodDescriptor {
            id: "openai_api_key".to_owned(),
            kind: AuthMethodKind::ApiKey,
            label: "API key".to_owned(),
            source_hint: None,
            supports_refresh: false,
        }
    ]));
}

#[test]
fn workspace_target_executables_reuse_built_helpers_when_available() {
    let workspace = Path::new("/tmp/belltower");
    let current_exe = workspace.join("target/debug/belltower");
    assert_eq!(
        workspace_helper_mode_for(&current_exe, Some(workspace), true, false),
        WorkspaceHelperMode::BuiltBinary
    );
}

#[test]
fn workspace_target_executables_fall_back_to_cargo_when_built_helper_missing() {
    let workspace = Path::new("/tmp/belltower");
    let current_exe = workspace.join("target/debug/belltower");
    assert_eq!(
        workspace_helper_mode_for(&current_exe, Some(workspace), false, false),
        WorkspaceHelperMode::CargoRun
    );
    assert_eq!(
        workspace_helper_mode_for(&current_exe, Some(workspace), true, true),
        WorkspaceHelperMode::CargoRun
    );
}

#[test]
fn installed_executables_do_not_force_workspace_helper_policy() {
    let workspace = Path::new("/tmp/belltower");
    let current_exe = Path::new("/usr/local/bin/belltower");
    assert_eq!(
        workspace_helper_mode_for(current_exe, Some(workspace), true, false),
        WorkspaceHelperMode::NotWorkspace
    );
}

#[test]
fn older_workspace_helper_is_reused_when_helper_sources_are_not_newer() {
    let temp_root = std::env::temp_dir().join(format!(
        "belltower-helper-freshness-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    fs::create_dir_all(&temp_root).expect("create temp root");
    let sibling = temp_root.join("bt-tui");
    let current = temp_root.join("belltower");
    fs::write(&sibling, b"older helper").expect("write sibling");
    std::thread::sleep(Duration::from_millis(20));
    fs::write(&current, b"newer launcher").expect("write current");

    assert!(sibling_binary_is_fresh_for(
        &current,
        &sibling,
        Some(&temp_root),
        "bt-tui"
    ));

    let _ = fs::remove_file(&sibling);
    let _ = fs::remove_file(&current);
    let _ = fs::remove_dir(&temp_root);
}

#[test]
fn stale_helper_source_forces_cargo_run_helpers() {
    let temp_root = std::env::temp_dir().join(format!(
        "belltower-helper-source-freshness-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let target_dir = temp_root.join("target/debug");
    let workspace = temp_root.clone();
    let crate_root = workspace.join("crates/bt-tui");
    let src_dir = crate_root.join("src");
    fs::create_dir_all(&target_dir).expect("create target dir");
    fs::create_dir_all(&src_dir).expect("create src dir");

    let current = target_dir.join("belltower");
    let sibling = target_dir.join("bt-tui");
    let source = src_dir.join("main.rs");
    fs::write(&current, b"launcher").expect("write launcher");
    fs::write(&sibling, b"helper").expect("write helper");
    std::thread::sleep(Duration::from_millis(20));
    fs::write(crate_root.join("Cargo.toml"), b"[package]\nname='bt-tui'\n")
        .expect("write manifest");
    fs::write(&source, b"fn main() {}\n").expect("write source");

    assert!(!sibling_binary_is_fresh_for(
        &current,
        &sibling,
        Some(&workspace),
        "bt-tui"
    ));

    let _ = fs::remove_file(&source);
    let _ = fs::remove_file(crate_root.join("Cargo.toml"));
    let _ = fs::remove_file(&sibling);
    let _ = fs::remove_file(&current);
    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn stale_helper_dependency_source_forces_cargo_run_helpers() {
    let temp_root = std::env::temp_dir().join(format!(
        "belltower-helper-dependency-freshness-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let target_dir = temp_root.join("target/debug");
    let workspace = temp_root.clone();
    let helper_root = workspace.join("crates/bt-tui");
    let dependency_root = workspace.join("crates/bt-client");
    fs::create_dir_all(&target_dir).expect("create target dir");
    fs::create_dir_all(helper_root.join("src")).expect("create helper src dir");
    fs::create_dir_all(dependency_root.join("src")).expect("create dependency src dir");

    let current = target_dir.join("belltower");
    let sibling = target_dir.join("bt-tui");
    fs::write(&current, b"launcher").expect("write launcher");
    fs::write(&sibling, b"helper").expect("write helper");
    fs::write(
        helper_root.join("Cargo.toml"),
        b"[package]\nname='bt-tui'\n[dependencies]\nbt-client = { path = '../bt-client' }\n",
    )
    .expect("write helper manifest");
    fs::write(helper_root.join("src/main.rs"), b"fn main() {}\n").expect("write helper source");
    fs::write(
        dependency_root.join("Cargo.toml"),
        b"[package]\nname='bt-client'\n",
    )
    .expect("write dependency manifest");
    std::thread::sleep(Duration::from_millis(20));
    fs::write(dependency_root.join("src/lib.rs"), b"pub fn changed() {}\n")
        .expect("write dependency source");

    assert!(!sibling_binary_is_fresh_for(
        &current,
        &sibling,
        Some(&workspace),
        "bt-tui"
    ));

    let _ = fs::remove_dir_all(&temp_root);
}

#[tokio::test]
async fn validate_connection_reports_healthy_for_openai_style_provider() {
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
        auth_sources: vec!["env:OPENAI_API_KEY".to_owned()],
        model_fallbacks: Vec::new(),
        discoverable_model_selectors: Vec::new(),
    };
    let credential = ResolvedCredential {
        provider: "openai".to_owned(),
        kind: CredentialKind::ApiKey,
        source: "auth store".to_owned(),
        tried_sources: Vec::new(),
        secret: "sk-test".to_owned(),
        refresh_secret: None,
        metadata: bt_core::CredentialMetadata::default(),
    };

    let probe = validate_connection(&connection, Some(&credential), Duration::from_secs(1))
        .await
        .expect("probe");
    assert!(probe.ready);
    assert!(probe.status.contains("healthy"));
    assert!(probe.status.contains("auth store"));
}

#[tokio::test]
async fn validate_connection_reports_degraded_for_http_error() {
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
    assert!(!probe.ready);
    assert!(probe.status.contains("degraded"));
    assert!(probe.status.contains("401"));
}

#[test]
fn normalize_local_base_url_strips_trailing_v1() {
    let backend = Url::parse("http://127.0.0.1:11434").expect("url");
    let connection = Url::parse("http://127.0.0.1:11434/v1").expect("url");
    assert_eq!(
        normalize_local_base_url(&backend),
        normalize_local_base_url(&connection)
    );
}

#[tokio::test]
async fn local_probe_flags_missing_configured_model() {
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

    let base_probe = validate_connection(&config.connections[0], None, Duration::from_secs(1))
        .await
        .expect("base probe");
    assert!(base_probe.ready);

    let probe = augment_local_probe(&config, &config.connections[0], base_probe)
        .await
        .expect("probe");
    assert!(!probe.ready);
    assert!(
        probe
            .status
            .contains("configured model `qwen3:latest` is not available")
    );
}

#[tokio::test]
async fn server_auth_compatible_checks_current_token_against_running_server() {
    async fn protected(headers: axum::http::HeaderMap) -> axum::http::StatusCode {
        let auth_ok = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == "Bearer test-token");
        if auth_ok {
            axum::http::StatusCode::OK
        } else {
            axum::http::StatusCode::UNAUTHORIZED
        }
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        serve(
            listener,
            Router::new().route("/connections", get(protected)),
        )
        .await
        .expect("server");
    });

    let token_file = NamedTempFile::new().expect("token file");
    fs::write(token_file.path(), "test-token").expect("write token");
    assert!(
        server_auth_compatible_with_token_path(&format!("http://{addr}/"), token_file.path()).await
    );

    fs::write(token_file.path(), "wrong-token").expect("write token");
    assert!(
        !server_auth_compatible_with_token_path(&format!("http://{addr}/"), token_file.path())
            .await
    );
}

#[tokio::test]
async fn server_protocol_ready_requires_health_and_authenticated_route() {
    async fn health() -> axum::http::StatusCode {
        axum::http::StatusCode::OK
    }

    async fn protected(headers: axum::http::HeaderMap) -> axum::http::StatusCode {
        let auth_ok = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == "Bearer test-token");
        if auth_ok {
            axum::http::StatusCode::OK
        } else {
            axum::http::StatusCode::UNAUTHORIZED
        }
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        serve(
            listener,
            Router::new()
                .route("/health", get(health))
                .route("/connections", get(protected)),
        )
        .await
        .expect("server");
    });

    let token_file = NamedTempFile::new().expect("token file");
    let token_paths = vec![token_file.path().to_path_buf()];

    fs::write(token_file.path(), "wrong-token").expect("write token");
    assert!(
        !server_protocol_ready_with_token_paths(&format!("http://{addr}/"), &token_paths).await
    );

    fs::write(token_file.path(), "test-token").expect("write token");
    assert!(server_protocol_ready_with_token_paths(&format!("http://{addr}/"), &token_paths).await);
}

#[test]
fn server_auth_token_candidate_paths_prefer_endpoint_token_before_legacy_token() {
    let paths =
        server_auth_token_candidate_paths("http://127.0.0.1:7400/").expect("candidate paths");

    assert_eq!(
        paths[0],
        bt_core::server_auth_token_path_for_url(
            &Url::parse("http://127.0.0.1:7400/").expect("url"),
        )
        .expect("endpoint path")
        .into_std_path_buf()
    );
    assert_eq!(paths[1], bt_core::auth_token_path().into_std_path_buf());
}
