#![forbid(unsafe_code)]

mod anthropic;
mod chatgpt;
mod openai;
mod sse;

use bt_core::{BelltowerError, ConnectionDescriptor, Result, RuntimeCredential, traits::Provider};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

/// One HTTP client (connection pool) shared by every provider instance.
/// Providers were previously constructed per turn, each with a fresh client,
/// paying a TLS handshake per turn. Streaming responses are read
/// incrementally, so only connect/read-stall guards are set here — never a
/// total request timeout, which would kill long completions.
pub(crate) fn shared_http_client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .read_timeout(Duration::from_secs(300))
                .pool_idle_timeout(Duration::from_secs(90))
                .tcp_keepalive(Duration::from_secs(60))
                .build()
                .expect("default TLS-capable HTTP client must construct")
        })
        .clone()
}

pub use anthropic::AnthropicProvider;
pub use chatgpt::{OpenAiChatGptProvider, chatgpt_account_id};
pub use openai::OpenAiCompatibleProvider;
pub use sse::{SseEvent, SseParser};

#[must_use]
pub fn connection_supported(connection: &ConnectionDescriptor) -> bool {
    matches!(
        connection.provider.as_str(),
        "openai-compatible" | "openai-chatgpt" | "anthropic"
    )
}

pub fn provider_for_connection(
    connection: &ConnectionDescriptor,
    credential: Option<RuntimeCredential>,
) -> Result<Arc<dyn Provider>> {
    match connection.provider.as_str() {
        "openai-compatible" => Ok(Arc::new(OpenAiCompatibleProvider::new(
            connection.id.to_string(),
            connection.base_url.clone(),
            openai_auth_token(credential)?,
        )?)),
        "openai-chatgpt" => {
            let (access_token, account_id) = openai_chatgpt_auth(credential)?;
            Ok(Arc::new(OpenAiChatGptProvider::new(
                connection.id.to_string(),
                connection.base_url.clone(),
                access_token,
                account_id,
            )?))
        }
        "anthropic" => Ok(Arc::new(AnthropicProvider::new(
            connection.id.to_string(),
            connection.base_url.clone(),
            anthropic_api_key(credential)?,
        )?)),
        other => Err(BelltowerError::Unsupported(format!(
            "provider `{other}` is not implemented yet"
        ))),
    }
}

fn openai_auth_token(credential: Option<RuntimeCredential>) -> Result<Option<String>> {
    match credential {
        None => Ok(None),
        Some(RuntimeCredential::ApiKey { secret }) => Ok(Some(secret)),
        Some(RuntimeCredential::BearerToken { access_token, .. }) => Ok(Some(access_token)),
        Some(other) => Err(BelltowerError::Unsupported(format!(
            "provider `openai-compatible` does not support {} runtime credentials",
            runtime_credential_label(&other)
        ))),
    }
}

fn anthropic_api_key(credential: Option<RuntimeCredential>) -> Result<Option<String>> {
    match credential {
        None => Ok(None),
        Some(RuntimeCredential::ApiKey { secret }) => Ok(Some(secret)),
        Some(other) => Err(BelltowerError::Unsupported(format!(
            "provider `anthropic` does not support {} runtime credentials",
            runtime_credential_label(&other)
        ))),
    }
}

fn openai_chatgpt_auth(credential: Option<RuntimeCredential>) -> Result<(String, String)> {
    match credential {
        Some(RuntimeCredential::BearerToken {
            access_token,
            metadata,
            ..
        }) => Ok((access_token, chatgpt_account_id(&metadata)?)),
        Some(other) => Err(BelltowerError::Unsupported(format!(
            "provider `openai-chatgpt` does not support {} runtime credentials",
            runtime_credential_label(&other)
        ))),
        None => Err(BelltowerError::Auth(
            "provider `openai-chatgpt` requires stored OAuth credentials; run `belltower login chatgpt`"
                .to_owned(),
        )),
    }
}

fn runtime_credential_label(credential: &RuntimeCredential) -> &'static str {
    match credential {
        RuntimeCredential::ApiKey { .. } => "api_key",
        RuntimeCredential::BearerToken { .. } => "bearer_token",
        RuntimeCredential::JsonDocument { .. } => "json_document",
    }
}

#[cfg(test)]
mod tests {
    use super::provider_for_connection;
    use bt_core::{
        ConnectionAuthMethodDescriptor, ConnectionDescriptor, ConnectionId, CredentialMetadata,
        RuntimeCredential,
    };
    use url::Url;

    fn test_connection(provider: &str) -> ConnectionDescriptor {
        ConnectionDescriptor {
            id: ConnectionId::new(provider),
            provider: provider.to_owned(),
            base_url: Url::parse("https://example.com/v1").expect("url"),
            default_model: "test-model".to_owned(),
            auth_methods: Vec::<ConnectionAuthMethodDescriptor>::new(),
            auth_sources: Vec::new(),
            model_fallbacks: Vec::new(),
            discoverable_model_selectors: Vec::new(),
        }
    }

    #[test]
    fn openai_compatible_accepts_bearer_tokens() {
        let provider = provider_for_connection(
            &test_connection("openai-compatible"),
            Some(RuntimeCredential::BearerToken {
                access_token: "oauth-access".to_owned(),
                refresh_token: Some("refresh".to_owned()),
                metadata: CredentialMetadata::default(),
            }),
        );
        assert!(provider.is_ok());
    }

    #[test]
    fn anthropic_rejects_non_api_key_runtime_credentials() {
        let result = provider_for_connection(
            &test_connection("anthropic"),
            Some(RuntimeCredential::BearerToken {
                access_token: "oauth-access".to_owned(),
                refresh_token: None,
                metadata: CredentialMetadata::default(),
            }),
        );
        let error = match result {
            Ok(_) => panic!("anthropic should reject bearer tokens"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("bearer_token"));
    }

    #[test]
    fn openai_chatgpt_requires_account_id_metadata() {
        let result = provider_for_connection(
            &test_connection("openai-chatgpt"),
            Some(RuntimeCredential::BearerToken {
                access_token: "oauth-access".to_owned(),
                refresh_token: Some("refresh".to_owned()),
                metadata: CredentialMetadata::default(),
            }),
        );
        let error = match result {
            Ok(_) => panic!("chatgpt should reject bearer tokens without account metadata"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("chatgpt_account_id"));
    }
}
