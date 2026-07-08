use super::{CredentialStore, resolved_from_credentials};
use bt_core::{BelltowerError, Credentials, Result};
use keyring::{Entry, Error as KeyringError};
use std::collections::BTreeSet;
use std::path::PathBuf;

const SERVICE_NAME: &str = "belltower";
const INDEX_ACCOUNT: &str = "__index__";

#[derive(Clone, Debug)]
pub struct KeychainStore {
    service: String,
}

impl Default for KeychainStore {
    fn default() -> Self {
        Self::new(SERVICE_NAME)
    }
}

impl KeychainStore {
    #[must_use]
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, connection_id: &str) -> Result<Entry> {
        Entry::new(&self.service, connection_id).map_err(keyring_error)
    }

    fn index_entry(&self) -> Result<Entry> {
        Entry::new(&self.service, INDEX_ACCOUNT).map_err(keyring_error)
    }

    fn load_index(&self) -> Result<BTreeSet<String>> {
        match self.index_entry()?.get_password() {
            Ok(value) => serde_json::from_str::<Vec<String>>(&value)
                .map(BTreeSet::from_iter)
                .map_err(|error| BelltowerError::Auth(error.to_string())),
            Err(KeyringError::NoEntry) => Ok(BTreeSet::new()),
            Err(error) => Err(keyring_error(error)),
        }
    }

    fn store_index(&self, index: &BTreeSet<String>) -> Result<()> {
        let entry = self.index_entry()?;
        if index.is_empty() {
            match entry.delete_credential() {
                Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
                Err(error) => Err(keyring_error(error)),
            }
        } else {
            let encoded = serde_json::to_string(&index.iter().cloned().collect::<Vec<_>>())
                .map_err(|error| BelltowerError::Auth(error.to_string()))?;
            entry.set_password(&encoded).map_err(keyring_error)
        }
    }

    #[must_use]
    pub fn service(&self) -> &str {
        &self.service
    }
}

impl CredentialStore for KeychainStore {
    fn load(&self, connection_id: &str) -> Result<Option<crate::ResolvedCredential>> {
        let raw = match self.entry(connection_id)?.get_password() {
            Ok(raw) => raw,
            Err(KeyringError::NoEntry) => return Ok(None),
            Err(error) => return Err(keyring_error(error)),
        };
        let credentials = serde_json::from_str::<Credentials>(&raw)
            .map_err(|error| BelltowerError::Auth(error.to_string()))?;
        Ok(Some(resolved_from_credentials(
            connection_id,
            format!(
                "keychain (service {}, account {})",
                self.service, connection_id
            ),
            credentials,
        )))
    }

    fn store(&self, connection_id: &str, credentials: Credentials) -> Result<()> {
        let encoded = serde_json::to_string(&credentials)
            .map_err(|error| BelltowerError::Auth(error.to_string()))?;
        self.entry(connection_id)?
            .set_password(&encoded)
            .map_err(keyring_error)?;
        let mut index = self.load_index()?;
        index.insert(connection_id.to_owned());
        self.store_index(&index)
    }

    fn delete(&self, connection_id: &str) -> Result<bool> {
        let entry = self.entry(connection_id)?;
        let deleted = match entry.delete_credential() {
            Ok(()) => true,
            Err(KeyringError::NoEntry) => false,
            Err(error) => return Err(keyring_error(error)),
        };
        let mut index = self.load_index()?;
        index.remove(connection_id);
        self.store_index(&index)?;
        Ok(deleted)
    }

    fn list(&self) -> Result<Vec<String>> {
        Ok(self.load_index()?.into_iter().collect())
    }

    fn label(&self) -> String {
        format!("keychain (service {})", self.service)
    }

    fn location(&self) -> Option<PathBuf> {
        None
    }
}

fn keyring_error(error: KeyringError) -> BelltowerError {
    BelltowerError::Auth(error.to_string())
}
