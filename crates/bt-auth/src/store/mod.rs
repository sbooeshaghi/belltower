mod auto;
mod ephemeral;
mod file;
#[cfg(feature = "keychain-backend")]
mod keychain;

pub use auto::AutoStore;
pub use ephemeral::{EphemeralEnvNames, EphemeralStore, ephemeral_env_names};
pub use file::FileStore;
#[cfg(feature = "keychain-backend")]
pub use keychain::KeychainStore;

use crate::ResolvedCredential;
use bt_core::{BelltowerError, CredentialMetadata, Credentials, Result};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CredentialStoreMode {
    #[default]
    Auto,
    File,
    Keychain,
    Ephemeral,
}

impl CredentialStoreMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::File => "file",
            Self::Keychain => "keychain",
            Self::Ephemeral => "ephemeral",
        }
    }
}

impl std::fmt::Display for CredentialStoreMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for CredentialStoreMode {
    type Err = BelltowerError;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "file" => Ok(Self::File),
            "keychain" => Ok(Self::Keychain),
            "ephemeral" => Ok(Self::Ephemeral),
            other => Err(BelltowerError::Config(format!(
                "unsupported auth storage `{other}`; expected auto, file, keychain, or ephemeral"
            ))),
        }
    }
}

#[derive(Clone)]
pub struct ConfiguredCredentialStore {
    mode: CredentialStoreMode,
    store: Arc<dyn CredentialStore>,
    summary: String,
}

impl ConfiguredCredentialStore {
    fn new(mode: CredentialStoreMode, store: Arc<dyn CredentialStore>, summary: String) -> Self {
        Self {
            mode,
            store,
            summary,
        }
    }

    #[must_use]
    pub const fn mode(&self) -> CredentialStoreMode {
        self.mode
    }

    #[must_use]
    pub fn store(&self) -> Arc<dyn CredentialStore> {
        self.store.clone()
    }

    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }
}

pub trait CredentialStore: Send + Sync {
    fn load(&self, connection_id: &str) -> Result<Option<ResolvedCredential>>;
    fn store(&self, connection_id: &str, credentials: Credentials) -> Result<()>;
    fn delete(&self, connection_id: &str) -> Result<bool>;
    fn list(&self) -> Result<Vec<String>>;
    fn label(&self) -> String;
    fn location(&self) -> Option<PathBuf>;
}

fn resolved_from_credentials(
    connection_id: &str,
    source: String,
    credentials: Credentials,
) -> ResolvedCredential {
    ResolvedCredential {
        provider: connection_id.to_owned(),
        kind: credentials.kind,
        source,
        tried_sources: Vec::new(),
        secret: credentials.secret,
        refresh_secret: credentials.refresh_secret,
        metadata: credentials.metadata,
    }
}

pub fn configured_credential_store(
    explicit_mode: Option<CredentialStoreMode>,
) -> Result<ConfiguredCredentialStore> {
    let mode = explicit_mode
        .or(env_override_store_mode()?)
        .unwrap_or_default();

    match mode {
        CredentialStoreMode::Auto => {
            let file = FileStore::default();
            let file_summary = file.label();
            #[cfg(feature = "keychain-backend")]
            let summary = format!(
                "auto (ephemeral env vars, keychain default write when available, {file_summary} fallback)"
            );
            #[cfg(not(feature = "keychain-backend"))]
            let summary =
                format!("auto (ephemeral env vars, {file_summary} default write/fallback)");
            Ok(ConfiguredCredentialStore::new(
                mode,
                Arc::new(AutoStore::default()),
                summary,
            ))
        }
        CredentialStoreMode::File => {
            let store = FileStore::default();
            let summary = store.label();
            Ok(ConfiguredCredentialStore::new(
                mode,
                Arc::new(store),
                summary,
            ))
        }
        CredentialStoreMode::Keychain => {
            #[cfg(feature = "keychain-backend")]
            {
                let store = KeychainStore::default();
                let summary = store.label();
                Ok(ConfiguredCredentialStore::new(
                    mode,
                    Arc::new(store),
                    summary,
                ))
            }
            #[cfg(not(feature = "keychain-backend"))]
            {
                Err(BelltowerError::Unsupported(
                    "keychain auth storage is not enabled in this build".to_owned(),
                ))
            }
        }
        CredentialStoreMode::Ephemeral => {
            let store = EphemeralStore;
            let summary = store.label();
            Ok(ConfiguredCredentialStore::new(
                mode,
                Arc::new(store),
                summary,
            ))
        }
    }
}

fn env_override_store_mode() -> Result<Option<CredentialStoreMode>> {
    match std::env::var("BELLTOWER_AUTH_STORE") {
        Ok(value) => CredentialStoreMode::from_str(&value).map(Some),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(BelltowerError::Config(error.to_string())),
    }
}

fn oauth_metadata_from_parts(
    account_id: Option<String>,
    plan_type: Option<String>,
    workspace_id: Option<String>,
) -> CredentialMetadata {
    CredentialMetadata {
        account_id: empty_to_none(account_id),
        plan_type: empty_to_none(plan_type),
        workspace_id: empty_to_none(workspace_id),
    }
}

fn empty_to_none(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}

impl<T> CredentialStore for Arc<T>
where
    T: CredentialStore + ?Sized,
{
    fn load(&self, connection_id: &str) -> Result<Option<ResolvedCredential>> {
        self.as_ref().load(connection_id)
    }

    fn store(&self, connection_id: &str, credentials: Credentials) -> Result<()> {
        self.as_ref().store(connection_id, credentials)
    }

    fn delete(&self, connection_id: &str) -> Result<bool> {
        self.as_ref().delete(connection_id)
    }

    fn list(&self) -> Result<Vec<String>> {
        self.as_ref().list()
    }

    fn label(&self) -> String {
        self.as_ref().label()
    }

    fn location(&self) -> Option<PathBuf> {
        self.as_ref().location()
    }
}
