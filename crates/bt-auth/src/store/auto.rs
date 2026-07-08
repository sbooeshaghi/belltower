use super::{CredentialStore, EphemeralStore, FileStore};
use bt_core::{Credentials, Result};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct AutoStore {
    ephemeral: Arc<dyn CredentialStore>,
    file: Arc<dyn CredentialStore>,
    #[cfg(feature = "keychain-backend")]
    keychain: Arc<dyn CredentialStore>,
}

impl Default for AutoStore {
    fn default() -> Self {
        Self::new(FileStore::default())
    }
}

impl AutoStore {
    #[must_use]
    pub fn new(file: FileStore) -> Self {
        #[cfg(feature = "keychain-backend")]
        {
            Self::with_backends(
                Arc::new(EphemeralStore),
                Arc::new(super::KeychainStore::default()),
                Arc::new(file),
            )
        }

        #[cfg(not(feature = "keychain-backend"))]
        {
            Self::with_backends(Arc::new(EphemeralStore), Arc::new(file))
        }
    }

    #[cfg(feature = "keychain-backend")]
    #[must_use]
    pub fn with_backends(
        ephemeral: Arc<dyn CredentialStore>,
        keychain: Arc<dyn CredentialStore>,
        file: Arc<dyn CredentialStore>,
    ) -> Self {
        Self {
            ephemeral,
            file,
            keychain,
        }
    }

    #[cfg(not(feature = "keychain-backend"))]
    #[must_use]
    pub fn with_backends(
        ephemeral: Arc<dyn CredentialStore>,
        file: Arc<dyn CredentialStore>,
    ) -> Self {
        Self { ephemeral, file }
    }

    fn preferred_write_label(&self) -> String {
        #[cfg(feature = "keychain-backend")]
        if self.keychain.list().is_ok() {
            return self.keychain.label();
        }
        self.file.label()
    }
}

impl CredentialStore for AutoStore {
    fn load(&self, connection_id: &str) -> Result<Option<crate::ResolvedCredential>> {
        let mut tried = vec![self.ephemeral.label()];
        if let Some(mut credential) = self.ephemeral.load(connection_id)? {
            credential.tried_sources = tried;
            return Ok(Some(credential));
        }

        #[cfg(feature = "keychain-backend")]
        {
            let keychain_label = self.keychain.label();
            match self.keychain.load(connection_id) {
                Ok(Some(mut credential)) => {
                    tried.push(keychain_label);
                    credential.tried_sources = tried;
                    return Ok(Some(credential));
                }
                Ok(None) => tried.push(keychain_label),
                Err(error) => tried.push(format!("{keychain_label} unavailable: {error}")),
            }
        }

        let file_label = self.file.label();
        let Some(mut credential) = self.file.load(connection_id)? else {
            return Ok(None);
        };
        tried.push(file_label);
        credential.tried_sources = tried;
        Ok(Some(credential))
    }

    fn store(&self, connection_id: &str, credentials: Credentials) -> Result<()> {
        #[cfg(feature = "keychain-backend")]
        {
            let keychain_entries = self.keychain.list();
            if keychain_entries
                .as_ref()
                .is_ok_and(|entries| entries.iter().any(|entry| entry == connection_id))
            {
                return self.keychain.store(connection_id, credentials);
            }
            if self.file.list()?.iter().any(|entry| entry == connection_id) {
                return self.file.store(connection_id, credentials);
            }
            if keychain_entries.is_ok() {
                return self.keychain.store(connection_id, credentials);
            }
        }
        self.file.store(connection_id, credentials)
    }

    fn delete(&self, connection_id: &str) -> Result<bool> {
        let mut deleted = false;
        #[cfg(feature = "keychain-backend")]
        if self.keychain.list().is_ok() {
            deleted |= self.keychain.delete(connection_id)?;
        }
        deleted |= self.file.delete(connection_id)?;
        Ok(deleted)
    }

    fn list(&self) -> Result<Vec<String>> {
        let mut providers = BTreeSet::new();
        providers.extend(self.ephemeral.list()?);
        #[cfg(feature = "keychain-backend")]
        if let Ok(entries) = self.keychain.list() {
            providers.extend(entries);
        }
        providers.extend(self.file.list()?);
        Ok(providers.into_iter().collect())
    }

    fn label(&self) -> String {
        format!("auto ({})", self.preferred_write_label())
    }

    fn location(&self) -> Option<PathBuf> {
        None
    }
}
