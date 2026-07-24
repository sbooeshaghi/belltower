#![forbid(unsafe_code)]

mod chatgpt;
mod store;

pub use chatgpt::{
    ChatGptDeviceCodeSession, ChatGptLoginOptions, OPENAI_CHATGPT_CLIENT_ID, OPENAI_CHATGPT_ISSUER,
    refresh_chatgpt_token_bundle,
};
#[cfg(feature = "keychain-backend")]
pub use store::KeychainStore;
pub use store::{
    AutoStore, ConfiguredCredentialStore, CredentialStore, CredentialStoreMode, EphemeralEnvNames,
    EphemeralStore, FileStore as FileAuthStore, configured_credential_store, ephemeral_env_names,
};

use bt_core::{
    BelltowerError, ConnectionDescriptor, CredentialKind, CredentialMetadata, Credentials,
    DetectionResult, Result, RuntimeCredential, auth_store_path,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

const AUTH_STORE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedCredential {
    pub provider: String,
    pub kind: CredentialKind,
    pub source: String,
    pub tried_sources: Vec<String>,
    pub secret: String,
    pub refresh_secret: Option<String>,
    pub metadata: CredentialMetadata,
}

impl ResolvedCredential {
    #[must_use]
    pub fn runtime_credential(&self) -> RuntimeCredential {
        match self.kind {
            CredentialKind::ApiKey => RuntimeCredential::ApiKey {
                secret: self.secret.clone(),
            },
            CredentialKind::OAuthToken => RuntimeCredential::BearerToken {
                access_token: self.secret.clone(),
                refresh_token: self.refresh_secret.clone(),
                metadata: self.metadata.clone(),
            },
            CredentialKind::JsonDocument => RuntimeCredential::JsonDocument {
                document: self.secret.clone(),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialInput {
    Literal(String),
    EnvReference(String),
    CommandReference(String),
}

impl CredentialInput {
    #[must_use]
    pub fn into_spec(self) -> String {
        match self {
            Self::Literal(secret) => secret,
            Self::EnvReference(env_var) => env_var,
            Self::CommandReference(command) => format!("!{command}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredSecretSource {
    Literal,
    EnvReference,
    CommandReference,
}

impl StoredSecretSource {
    fn from_input(input: &CredentialInput) -> Self {
        match input {
            CredentialInput::Literal(_) => Self::Literal,
            CredentialInput::EnvReference(_) => Self::EnvReference,
            CredentialInput::CommandReference(_) => Self::CommandReference,
        }
    }
}

#[derive(Clone)]
pub struct CredentialResolver {
    store: Arc<dyn CredentialStore>,
}

impl CredentialResolver {
    pub fn new() -> Result<Self> {
        Ok(Self::with_credential_store(
            configured_credential_store(None)?.store(),
        ))
    }

    #[must_use]
    pub fn with_store<T>(store: T) -> Self
    where
        T: CredentialStore + 'static,
    {
        Self::with_credential_store(Arc::new(store))
    }

    #[must_use]
    pub fn with_credential_store(store: Arc<dyn CredentialStore>) -> Self {
        Self { store }
    }

    #[must_use]
    pub fn store(&self) -> Arc<dyn CredentialStore> {
        self.store.clone()
    }

    pub fn resolve(&self, connection: &ConnectionDescriptor) -> Result<Option<ResolvedCredential>> {
        if let Some(credential) = self.resolve_from_auth_store(connection)? {
            return Ok(Some(credential));
        }

        for source in &connection.auth_sources {
            if let Some(credential) = self.resolve_source(connection, source)? {
                return Ok(Some(credential));
            }
        }

        Ok(None)
    }

    pub async fn resolve_fresh(
        &self,
        connection: &ConnectionDescriptor,
    ) -> Result<Option<ResolvedCredential>> {
        if let Some(credential) = self.resolve_from_auth_store_fresh(connection).await? {
            return Ok(Some(credential));
        }

        for source in &connection.auth_sources {
            if let Some(credential) = self.resolve_source(connection, source)? {
                return Ok(Some(credential));
            }
        }

        Ok(None)
    }

    pub fn resolve_api_key(
        &self,
        connection: &ConnectionDescriptor,
    ) -> Result<Option<ResolvedCredential>> {
        Ok(self
            .resolve(connection)?
            .filter(|credential| credential.kind == CredentialKind::ApiKey))
    }

    pub fn resolve_runtime_credential(
        &self,
        connection: &ConnectionDescriptor,
    ) -> Result<Option<RuntimeCredential>> {
        Ok(self
            .resolve(connection)?
            .map(|credential| credential.runtime_credential()))
    }

    pub async fn resolve_runtime_credential_fresh(
        &self,
        connection: &ConnectionDescriptor,
    ) -> Result<Option<RuntimeCredential>> {
        Ok(self
            .resolve_fresh(connection)
            .await?
            .map(|credential| credential.runtime_credential()))
    }

    pub fn detect(&self, connection: &ConnectionDescriptor) -> Result<DetectionResult> {
        let mut sources = Vec::new();
        if let Some(credential) = self.resolve_from_auth_store(connection)? {
            sources.push(credential.source);
        }
        for source in &connection.auth_sources {
            if self.resolve_source(connection, source)?.is_some() {
                sources.push(source_label(source));
            }
        }
        Ok(DetectionResult {
            provider: connection.id.to_string(),
            found: !sources.is_empty(),
            sources,
        })
    }

    fn resolve_from_auth_store(
        &self,
        connection: &ConnectionDescriptor,
    ) -> Result<Option<ResolvedCredential>> {
        self.store.load(connection.id.0.as_str())
    }

    async fn resolve_from_auth_store_fresh(
        &self,
        connection: &ConnectionDescriptor,
    ) -> Result<Option<ResolvedCredential>> {
        let Some(mut credential) = self.resolve_from_auth_store(connection)? else {
            return Ok(None);
        };

        if chatgpt_refresh_needed(connection, &credential)? {
            // ChatGPT rotates refresh tokens on use: two concurrent refreshes
            // burn the stored refresh token permanently. Serialize per
            // provider, then re-check — the winner of the race has usually
            // already refreshed by the time a waiter acquires the lock.
            let refresh_lock = oauth_refresh_lock(connection.id.0.as_str());
            let _refresh_guard = refresh_lock.lock().await;
            credential = match self.resolve_from_auth_store(connection)? {
                Some(credential) => credential,
                None => return Ok(None),
            };
            if chatgpt_refresh_needed(connection, &credential)? {
                if let Err(error) = refresh_chatgpt_token_bundle(
                    ChatGptLoginOptions::for_provider(connection.id.to_string())
                        .with_shared_store(self.store.clone()),
                )
                .await
                {
                    if let Some(reloaded) = self.resolve_from_auth_store(connection)?
                        && reloaded_chatgpt_credential_is_usable(
                            connection,
                            &credential,
                            &reloaded,
                        )?
                    {
                        return Ok(Some(reloaded));
                    }
                    return Err(error);
                }
                credential = match self.resolve_from_auth_store(connection)? {
                    Some(credential) => credential,
                    None => {
                        return Err(BelltowerError::Auth(format!(
                            "refreshed ChatGPT credentials for `{}` but no credential was reloaded from the auth store",
                            connection.id
                        )));
                    }
                };
            }
        }

        Ok(Some(credential))
    }

    fn resolve_source(
        &self,
        connection: &ConnectionDescriptor,
        source: &str,
    ) -> Result<Option<ResolvedCredential>> {
        match parse_source(source) {
            ParsedSource::EnvVar(name) => resolve_env_var(&name).map(|secret| {
                secret.map(|secret| ResolvedCredential {
                    provider: connection.id.to_string(),
                    kind: CredentialKind::ApiKey,
                    source: name,
                    tried_sources: Vec::new(),
                    secret,
                    refresh_secret: None,
                    metadata: CredentialMetadata::default(),
                })
            }),
            ParsedSource::File(path) => {
                let path = expand_path(&path);
                if !path.exists() {
                    return Ok(None);
                }
                Ok(
                    load_secret_from_file(&path)?.map(|secret| ResolvedCredential {
                        provider: connection.id.to_string(),
                        kind: CredentialKind::ApiKey,
                        source: path.display().to_string(),
                        tried_sources: Vec::new(),
                        secret,
                        refresh_secret: None,
                        metadata: CredentialMetadata::default(),
                    }),
                )
            }
            ParsedSource::Special(name) if name == "gcloud-adc" => resolve_gcloud_adc(connection),
            ParsedSource::Special(_) => Ok(None),
        }
    }
}

/// Process-wide per-provider lock serializing OAuth token refreshes.
fn oauth_refresh_lock(provider: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    LOCKS
        .get_or_init(Default::default)
        .lock()
        .expect("oauth refresh lock registry poisoned")
        .entry(provider.to_owned())
        .or_default()
        .clone()
}

fn reloaded_chatgpt_credential_is_usable(
    connection: &ConnectionDescriptor,
    previous: &ResolvedCredential,
    reloaded: &ResolvedCredential,
) -> Result<bool> {
    if connection.provider != "openai-chatgpt"
        || reloaded.kind != CredentialKind::OAuthToken
        || (reloaded.secret == previous.secret
            && reloaded.refresh_secret == previous.refresh_secret)
    {
        return Ok(false);
    }
    Ok(!chatgpt_refresh_needed(connection, reloaded)?)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ParsedSource {
    EnvVar(String),
    File(String),
    Special(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct StoredCredential {
    kind: CredentialKind,
    secret: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_secret: Option<String>,
    #[serde(default, skip_serializing_if = "CredentialMetadata::is_empty")]
    metadata: CredentialMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<StoredSecretSource>,
}

impl From<Credentials> for StoredCredential {
    fn from(value: Credentials) -> Self {
        Self {
            kind: value.kind,
            secret: value.secret,
            refresh_secret: value.refresh_secret,
            metadata: value.metadata,
            source: None,
        }
    }
}

impl StoredCredential {
    fn from_input(input: CredentialInput) -> Self {
        let source = StoredSecretSource::from_input(&input);
        Self {
            kind: CredentialKind::ApiKey,
            secret: input.into_spec(),
            refresh_secret: None,
            metadata: CredentialMetadata::default(),
            source: Some(source),
        }
    }

    fn resolve_secret(&self) -> Result<Option<String>> {
        match self.source.as_ref() {
            Some(StoredSecretSource::Literal) => resolve_literal_secret(&self.secret),
            Some(StoredSecretSource::EnvReference) => resolve_env_var(&self.secret),
            Some(StoredSecretSource::CommandReference) => resolve_command_reference(&self.secret),
            None => resolve_secret_spec(&self.secret),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct AuthStoreFile {
    #[serde(default = "store_version")]
    version: u32,
    #[serde(default)]
    active_connection: Option<String>,
    #[serde(default)]
    providers: BTreeMap<String, StoredCredential>,
}

fn store_version() -> u32 {
    AUTH_STORE_VERSION
}

static COMMAND_CACHE: OnceLock<Mutex<BTreeMap<String, Option<String>>>> = OnceLock::new();

pub fn require_api_key(
    resolver: &CredentialResolver,
    connection: &ConnectionDescriptor,
) -> Result<Option<ResolvedCredential>> {
    let credential = resolver.resolve_api_key(connection)?;
    if credential.is_none() && !connection.auth_sources.is_empty() {
        return Err(BelltowerError::Auth(format!(
            "no API key credentials found for connection `{}`; checked auth store and {}",
            connection.id,
            connection.auth_sources.join(", ")
        )));
    }

    Ok(credential)
}

pub fn require_runtime_credential(
    resolver: &CredentialResolver,
    connection: &ConnectionDescriptor,
) -> Result<Option<RuntimeCredential>> {
    let credential = resolver.resolve_runtime_credential(connection)?;
    if credential.is_none() && !connection.auth_sources.is_empty() {
        return Err(BelltowerError::Auth(format!(
            "no credentials found for connection `{}`; checked auth store and {}",
            connection.id,
            connection.auth_sources.join(", ")
        )));
    }

    Ok(credential)
}

pub async fn require_runtime_credential_fresh(
    resolver: &CredentialResolver,
    connection: &ConnectionDescriptor,
) -> Result<Option<RuntimeCredential>> {
    let credential = resolver
        .resolve_runtime_credential_fresh(connection)
        .await?;
    if credential.is_none() && !connection.auth_sources.is_empty() {
        return Err(BelltowerError::Auth(format!(
            "no credentials found for connection `{}`; checked auth store and {}",
            connection.id,
            connection.auth_sources.join(", ")
        )));
    }

    Ok(credential)
}

#[must_use]
pub fn managed_auth_store_path() -> PathBuf {
    auth_store_path().into()
}

pub fn auth_storage_summary(explicit_mode: Option<CredentialStoreMode>) -> Result<String> {
    Ok(configured_credential_store(explicit_mode)?
        .summary()
        .to_owned())
}

pub fn store_api_key(provider: &str, api_key: &str) -> Result<String> {
    store_credential(
        provider,
        CredentialInput::Literal(api_key.trim().to_owned()),
        None,
    )
}

pub fn store_credential(
    provider: &str,
    input: CredentialInput,
    explicit_mode: Option<CredentialStoreMode>,
) -> Result<String> {
    let configured = configured_credential_store(explicit_mode)?;
    match configured.mode() {
        CredentialStoreMode::File => {
            let file = FileAuthStore::default();
            file.store_credential_input_sync(provider, input)?;
        }
        _ => {
            let Some(secret) = resolve_secret_spec(&input.into_spec())? else {
                return Err(BelltowerError::Auth(format!(
                    "no secret value resolved for `{provider}`"
                )));
            };
            configured.store().store(
                provider,
                Credentials {
                    provider: provider.to_owned(),
                    kind: CredentialKind::ApiKey,
                    secret,
                    refresh_secret: None,
                    metadata: CredentialMetadata::default(),
                },
            )?;
        }
    }
    Ok(configured.summary().to_owned())
}

pub fn store_oauth_token_bundle(
    provider: &str,
    access_token: impl Into<String>,
    refresh_token: Option<String>,
    metadata: CredentialMetadata,
) -> Result<String> {
    store_oauth_token_bundle_with_mode(provider, access_token, refresh_token, metadata, None)
}

pub fn store_oauth_token_bundle_with_mode(
    provider: &str,
    access_token: impl Into<String>,
    refresh_token: Option<String>,
    metadata: CredentialMetadata,
    explicit_mode: Option<CredentialStoreMode>,
) -> Result<String> {
    let configured = configured_credential_store(explicit_mode)?;
    configured.store().store(
        provider,
        Credentials {
            provider: provider.to_owned(),
            kind: CredentialKind::OAuthToken,
            secret: access_token.into(),
            refresh_secret: refresh_token,
            metadata,
        },
    )?;
    Ok(configured.summary().to_owned())
}

pub fn clear_credentials(
    provider: &str,
    explicit_mode: Option<CredentialStoreMode>,
) -> Result<bool> {
    configured_credential_store(explicit_mode)?
        .store()
        .delete(provider)
}

fn chatgpt_refresh_needed(
    connection: &ConnectionDescriptor,
    credential: &ResolvedCredential,
) -> Result<bool> {
    if connection.provider != "openai-chatgpt"
        || credential.kind != CredentialKind::OAuthToken
        || credential.refresh_secret.is_none()
    {
        return Ok(false);
    }

    match chatgpt::access_token_refresh_required(
        &credential.secret,
        std::time::Duration::from_secs(300),
    ) {
        Ok(refresh_required) => Ok(refresh_required),
        // ChatGPT access-token shape is an external contract. If it becomes
        // opaque or otherwise unreadable, prefer the refresh token over
        // attempting provider calls with a credential we cannot prove fresh.
        Err(_) => Ok(true),
    }
}

fn parse_source(source: &str) -> ParsedSource {
    if let Some(env_var) = source.strip_prefix("env:") {
        return ParsedSource::EnvVar(env_var.to_owned());
    }
    if let Some(path) = source.strip_prefix("file:") {
        return ParsedSource::File(path.to_owned());
    }
    if let Some(name) = source.strip_prefix("special:") {
        return ParsedSource::Special(name.to_owned());
    }
    if source == "gcloud-adc" {
        return ParsedSource::Special(source.to_owned());
    }
    if looks_like_env_var(source) {
        return ParsedSource::EnvVar(source.to_owned());
    }
    ParsedSource::File(source.to_owned())
}

fn source_label(source: &str) -> String {
    match parse_source(source) {
        ParsedSource::EnvVar(name) => name,
        ParsedSource::File(path) => expand_path(&path).display().to_string(),
        ParsedSource::Special(name) if name == "gcloud-adc" => "gcloud-adc".to_owned(),
        ParsedSource::Special(name) => name,
    }
}

fn describe_stored_secret_source(source: Option<&StoredSecretSource>, secret: &str) -> String {
    match source {
        Some(StoredSecretSource::Literal) => "literal".to_owned(),
        Some(StoredSecretSource::EnvReference) => format!("env:{secret}"),
        Some(StoredSecretSource::CommandReference) => "command".to_owned(),
        None => describe_spec_source(secret),
    }
}

fn describe_spec_source(spec: &str) -> String {
    if spec.starts_with('!') {
        "command".to_owned()
    } else if looks_like_env_var(spec) {
        format!("env:{spec}")
    } else {
        "literal".to_owned()
    }
}

fn resolve_env_var(name: &str) -> Result<Option<String>> {
    match env::var(name) {
        Ok(secret) => {
            let secret = secret.trim();
            if secret.is_empty() {
                Ok(None)
            } else {
                Ok(Some(secret.to_owned()))
            }
        }
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(BelltowerError::Auth(error.to_string())),
    }
}

fn resolve_secret_spec(spec: &str) -> Result<Option<String>> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if let Some(command) = trimmed.strip_prefix('!') {
        return resolve_command_reference(command);
    }
    if looks_like_env_var(trimmed) {
        return resolve_env_var(trimmed);
    }
    Ok(Some(trimmed.to_owned()))
}

fn resolve_literal_secret(secret: &str) -> Result<Option<String>> {
    let trimmed = secret.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_owned()))
    }
}

fn resolve_command_reference(command: &str) -> Result<Option<String>> {
    let cache = COMMAND_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    {
        let cache = cache
            .lock()
            .map_err(|error| BelltowerError::Auth(error.to_string()))?;
        if let Some(result) = cache.get(command) {
            return Ok(result.clone());
        }
    }

    let output = shell_command(command).output();
    let resolved = match output {
        Ok(output) if output.status.success() => {
            let secret = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if secret.is_empty() {
                None
            } else {
                Some(secret)
            }
        }
        Ok(_) => None,
        Err(error) => return Err(BelltowerError::Auth(error.to_string())),
    };

    let mut cache = cache
        .lock()
        .map_err(|error| BelltowerError::Auth(error.to_string()))?;
    cache.insert(command.to_owned(), resolved.clone());
    Ok(resolved)
}

#[cfg(unix)]
fn shell_command(command: &str) -> Command {
    let mut shell = Command::new("sh");
    shell.arg("-lc").arg(command);
    shell
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    let mut shell = Command::new("cmd");
    shell.arg("/C").arg(command);
    shell
}

fn looks_like_env_var(source: &str) -> bool {
    !source.contains('/')
        && !source.contains('\\')
        && source
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
}

fn expand_path(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/")
        && let Ok(home) = env::var("HOME")
    {
        return PathBuf::from(home).join(stripped);
    }

    PathBuf::from(path)
}

fn load_secret_from_file(path: &Path) -> Result<Option<String>> {
    let raw = fs::read_to_string(path)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    if (path.extension().is_some_and(|ext| ext == "json") || trimmed.starts_with('{'))
        && let Ok(value) = serde_json::from_str::<Value>(trimmed)
        && let Some(secret) = extract_secret(&value)
    {
        return Ok(Some(secret));
    }

    Ok(Some(trimmed.to_owned()))
}

fn resolve_gcloud_adc(connection: &ConnectionDescriptor) -> Result<Option<ResolvedCredential>> {
    let Some(path) = gcloud_adc_path() else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)?;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(ResolvedCredential {
        provider: connection.id.to_string(),
        kind: CredentialKind::JsonDocument,
        source: path.display().to_string(),
        tried_sources: Vec::new(),
        secret: raw,
        refresh_secret: None,
        metadata: CredentialMetadata::default(),
    }))
}

fn gcloud_adc_path() -> Option<PathBuf> {
    if let Ok(path) = env::var("GOOGLE_APPLICATION_CREDENTIALS") {
        let path = path.trim();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    #[cfg(windows)]
    {
        if let Ok(appdata) = env::var("APPDATA") {
            return Some(
                PathBuf::from(appdata).join("gcloud/application_default_credentials.json"),
            );
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(home) = env::var("HOME") {
            return Some(
                PathBuf::from(home).join(".config/gcloud/application_default_credentials.json"),
            );
        }
    }
    None
}

fn extract_secret(value: &Value) -> Option<String> {
    const DIRECT_KEYS: &[&str] = &[
        "api_key",
        "apikey",
        "key",
        "token",
        "access_token",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "GOOGLE_API_KEY",
    ];

    match value {
        Value::Object(map) => {
            for key in DIRECT_KEYS {
                if let Some(secret) = map.get(*key).and_then(Value::as_str) {
                    let secret = secret.trim();
                    if !secret.is_empty() {
                        return Some(secret.to_owned());
                    }
                }
            }

            for nested in map.values() {
                if let Some(secret) = extract_secret(nested) {
                    return Some(secret);
                }
            }

            None
        }
        Value::Array(items) => items.iter().find_map(extract_secret),
        Value::String(secret) if !secret.trim().is_empty() => Some(secret.trim().to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CredentialInput, CredentialResolver, CredentialStore, FileAuthStore, ResolvedCredential,
        extract_secret, reloaded_chatgpt_credential_is_usable, resolve_secret_spec,
    };
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use bt_core::{
        ConnectionDescriptor, ConnectionId, CredentialKind, CredentialMetadata, Credentials,
        RuntimeCredential,
    };
    use serde_json::json;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;
    use url::Url;

    fn temp_store() -> (TempDir, FileAuthStore) {
        let temp = TempDir::new().expect("tempdir");
        let store = FileAuthStore::new(temp.path().join("auth.json"));
        (temp, store)
    }

    fn test_connection(auth_sources: Vec<String>) -> ConnectionDescriptor {
        ConnectionDescriptor {
            id: ConnectionId::new("openai"),
            provider: "openai-compatible".to_owned(),
            base_url: Url::parse("https://api.openai.com/v1").expect("url"),
            default_model: "o4-mini".to_owned(),
            auth_methods: Vec::new(),
            auth_sources,
            model_fallbacks: Vec::new(),
            discoverable_model_selectors: Vec::new(),
        }
    }

    fn chatgpt_connection() -> ConnectionDescriptor {
        ConnectionDescriptor {
            id: ConnectionId::new("chatgpt"),
            provider: "openai-chatgpt".to_owned(),
            base_url: Url::parse("https://chatgpt.com/backend-api").expect("url"),
            default_model: "gpt-5.4-mini".to_owned(),
            auth_methods: Vec::new(),
            auth_sources: vec!["auth-store".to_owned()],
            model_fallbacks: Vec::new(),
            discoverable_model_selectors: Vec::new(),
        }
    }

    fn oauth_credential(access_token: String, refresh_token: &str) -> ResolvedCredential {
        ResolvedCredential {
            provider: "chatgpt".to_owned(),
            kind: CredentialKind::OAuthToken,
            source: "auth store".to_owned(),
            tried_sources: Vec::new(),
            secret: access_token,
            refresh_secret: Some(refresh_token.to_owned()),
            metadata: CredentialMetadata::default(),
        }
    }

    fn fake_jwt(claims: serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&claims).expect("serialize fake token claims"));
        format!("{header}.{payload}.signature")
    }

    fn fake_access_token(exp_offset_secs: i64) -> String {
        let now_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        fake_jwt(json!({ "exp": now_epoch + exp_offset_secs, "chatgpt_account_id": "acct" }))
    }

    #[test]
    fn extracts_nested_secret_keys() {
        let value = json!({
            "provider": {
                "token": "secret-value"
            }
        });
        assert_eq!(extract_secret(&value).as_deref(), Some("secret-value"));
    }

    #[test]
    fn auth_store_preserves_literal_env_like_secret() {
        let (_temp, store) = temp_store();
        store_credential_for_test(
            &store,
            "openai",
            CredentialInput::Literal("HOME".to_owned()),
        );

        let credential = store
            .get_credentials_sync("openai")
            .expect("load")
            .expect("credential");
        assert_eq!(credential.kind, CredentialKind::ApiKey);
        assert_eq!(credential.secret, "HOME");
        assert_eq!(credential.refresh_secret, None);
        assert!(credential.metadata.is_empty());

        let resolver = CredentialResolver::with_store(store);
        let connection = test_connection(Vec::new());
        let resolved = resolver
            .resolve_api_key(&connection)
            .expect("resolve")
            .expect("credential");
        assert_eq!(resolved.secret, "HOME");
        assert!(resolved.source.contains("literal"));
    }

    #[test]
    fn resolver_prefers_auth_store_over_env_sources() {
        let (_temp, store) = temp_store();
        store_credential_for_test(
            &store,
            "openai",
            CredentialInput::Literal("sk-store".to_owned()),
        );
        let resolver = CredentialResolver::with_store(store);
        let connection = test_connection(vec!["env:HOME".to_owned()]);

        let credential = resolver
            .resolve_api_key(&connection)
            .expect("resolve")
            .expect("credential");
        assert_eq!(credential.secret, "sk-store");
        assert!(credential.source.contains("auth store"));
    }

    #[test]
    fn env_reference_in_auth_store_is_resolved() {
        let (_temp, store) = temp_store();
        store_credential_for_test(
            &store,
            "openai",
            CredentialInput::EnvReference("HOME".to_owned()),
        );
        let resolver = CredentialResolver::with_store(store);
        let connection = test_connection(Vec::new());

        let credential = resolver
            .resolve_api_key(&connection)
            .expect("resolve")
            .expect("credential");
        assert!(!credential.secret.is_empty());
        assert!(credential.source.contains("env:HOME"));
    }

    #[test]
    fn legacy_auth_store_entries_still_resolve_env_references() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("auth.json");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "providers": {
                    "openai": {
                        "kind": "ApiKey",
                        "secret": "HOME"
                    }
                }
            }))
            .expect("json"),
        )
        .expect("write");

        let resolver = CredentialResolver::with_store(FileAuthStore::new(&path));
        let connection = test_connection(Vec::new());
        let credential = resolver
            .resolve_api_key(&connection)
            .expect("resolve")
            .expect("credential");
        assert_eq!(credential.secret, std::env::var("HOME").expect("home"));
    }

    #[test]
    fn command_reference_is_cached_per_process() {
        let temp = TempDir::new().expect("tempdir");
        let counter = temp.path().join("counter.txt");
        fs::write(&counter, "0\n").expect("counter");

        let command = format!(
            "count=$(cat '{}'); next=$((count + 1)); echo \"$next\" > '{}'; printf 'sk-cached'",
            counter.display(),
            counter.display()
        );
        let first = resolve_secret_spec(&format!("!{command}"))
            .expect("resolve")
            .expect("first");
        let second = resolve_secret_spec(&format!("!{command}"))
            .expect("resolve")
            .expect("second");

        assert_eq!(first, "sk-cached");
        assert_eq!(second, "sk-cached");
        assert_eq!(fs::read_to_string(counter).expect("counter").trim(), "1");
    }

    #[test]
    fn file_sources_extract_nested_secrets() {
        let temp = TempDir::new().expect("tempdir");
        let json_path = temp.path().join("anthropic.json");
        fs::write(
            &json_path,
            serde_json::to_vec(&json!({
                "tokens": { "access_token": "sk-file" }
            }))
            .expect("json"),
        )
        .expect("write");

        let resolver =
            CredentialResolver::with_store(FileAuthStore::new(temp.path().join("auth-store.json")));
        let connection = test_connection(vec![format!("file:{}", json_path.display())]);
        let credential = resolver
            .resolve_api_key(&connection)
            .expect("resolve")
            .expect("credential");
        assert_eq!(credential.secret, "sk-file");
    }

    #[test]
    fn clear_credentials_removes_entry() {
        let (_temp, store) = temp_store();
        store_credential_for_test(
            &store,
            "openai",
            CredentialInput::Literal("sk-test".to_owned()),
        );
        assert!(store.clear_credentials_sync("openai").expect("clear"));
        assert!(
            store
                .get_credentials_sync("openai")
                .expect("load")
                .is_none()
        );
    }

    #[test]
    fn read_only_auth_checks_do_not_create_store_file() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("auth.json");
        let store = FileAuthStore::new(&path);

        assert!(
            store
                .get_credentials_sync("openai")
                .expect("lookup")
                .is_none()
        );
        assert!(!path.exists());

        let detection = store.detect_credentials_sync("openai").expect("detect");
        assert!(!detection.found);
        assert!(!path.exists());
    }

    #[test]
    fn file_store_satisfies_credential_store_trait_contract() {
        let (_temp, file_store) = temp_store();
        let store: Arc<dyn CredentialStore> = Arc::new(file_store.clone());

        store
            .store(
                "openai",
                Credentials {
                    provider: "openai".to_owned(),
                    kind: CredentialKind::ApiKey,
                    secret: "sk-trait".to_owned(),
                    refresh_secret: None,
                    metadata: CredentialMetadata::default(),
                },
            )
            .expect("store through trait");

        let loaded = store
            .load("openai")
            .expect("load through trait")
            .expect("credential");
        assert_eq!(loaded.provider, "openai");
        assert_eq!(loaded.secret, "sk-trait");
        assert!(loaded.source.contains("auth store"));

        let listed = store.list().expect("list through trait");
        assert_eq!(listed, vec!["openai".to_owned()]);
        assert_eq!(store.location(), Some(file_store.path().to_path_buf()));
        assert!(store.label().contains("auth store"));

        assert!(store.delete("openai").expect("delete through trait"));
        assert!(store.load("openai").expect("load after delete").is_none());
    }

    #[test]
    fn runtime_credential_maps_oauth_tokens_from_auth_store() {
        let (_temp, store) = temp_store();
        store
            .store_credentials_sync(
                "openai",
                Credentials {
                    provider: "openai".to_owned(),
                    kind: CredentialKind::OAuthToken,
                    secret: "oauth-access".to_owned(),
                    refresh_secret: Some("oauth-refresh".to_owned()),
                    metadata: CredentialMetadata {
                        account_id: Some("acct-123".to_owned()),
                        plan_type: Some("pro".to_owned()),
                        workspace_id: Some("ws-abc".to_owned()),
                    },
                },
            )
            .expect("store oauth credential");

        let resolver = CredentialResolver::with_store(store);
        let credential = resolver
            .resolve_runtime_credential(&test_connection(Vec::new()))
            .expect("resolve runtime credential")
            .expect("credential");

        assert_eq!(
            credential,
            RuntimeCredential::BearerToken {
                access_token: "oauth-access".to_owned(),
                refresh_token: Some("oauth-refresh".to_owned()),
                metadata: CredentialMetadata {
                    account_id: Some("acct-123".to_owned()),
                    plan_type: Some("pro".to_owned()),
                    workspace_id: Some("ws-abc".to_owned()),
                },
            }
        );
    }

    #[test]
    fn chatgpt_refresh_failure_can_use_newer_fresh_store_credential() {
        let connection = chatgpt_connection();
        let previous = oauth_credential(fake_access_token(-60), "refresh-token-0");
        let reloaded = oauth_credential(fake_access_token(3600), "refresh-token-1");

        assert!(
            reloaded_chatgpt_credential_is_usable(&connection, &previous, &reloaded)
                .expect("reload check")
        );
    }

    #[test]
    fn chatgpt_refresh_failure_rejects_unchanged_or_stale_store_credential() {
        let connection = chatgpt_connection();
        let previous = oauth_credential(fake_access_token(-60), "refresh-token-0");
        let unchanged = previous.clone();
        let still_stale = oauth_credential(fake_access_token(-30), "refresh-token-1");

        assert!(
            !reloaded_chatgpt_credential_is_usable(&connection, &previous, &unchanged)
                .expect("unchanged reload check")
        );
        assert!(
            !reloaded_chatgpt_credential_is_usable(&connection, &previous, &still_stale)
                .expect("stale reload check")
        );
    }

    #[test]
    fn auth_store_round_trips_oauth_bundle_metadata() {
        let (_temp, store) = temp_store();
        let metadata = CredentialMetadata {
            account_id: Some("acct-chatgpt".to_owned()),
            plan_type: Some("plus".to_owned()),
            workspace_id: Some("ws-main".to_owned()),
        };
        store
            .store_credentials_sync(
                "openai-chatgpt",
                Credentials {
                    provider: "openai-chatgpt".to_owned(),
                    kind: CredentialKind::OAuthToken,
                    secret: "oauth-access".to_owned(),
                    refresh_secret: Some("oauth-refresh".to_owned()),
                    metadata: metadata.clone(),
                },
            )
            .expect("store oauth bundle");
        let stored = store
            .get_credentials_sync("openai-chatgpt")
            .expect("load")
            .expect("credential");
        assert_eq!(stored.kind, CredentialKind::OAuthToken);
        assert_eq!(stored.secret, "oauth-access");
        assert_eq!(stored.refresh_secret.as_deref(), Some("oauth-refresh"));
        assert_eq!(stored.metadata, metadata);
    }

    fn store_credential_for_test(store: &FileAuthStore, provider: &str, input: CredentialInput) {
        store
            .store_credential_input_sync(provider, input)
            .expect("store credential");
    }
}
