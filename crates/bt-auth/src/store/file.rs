use super::CredentialStore;
use crate::{
    AuthStoreFile, CredentialInput, ResolvedCredential, StoredCredential,
    describe_stored_secret_source,
};
use bt_core::{
    BelltowerError, Credentials, DetectionResult, Result, auth_store_path, traits::AuthStore,
};
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct FileStore {
    path: PathBuf,
}

impl Default for FileStore {
    fn default() -> Self {
        Self::new(auth_store_path().into_std_path_buf())
    }
}

impl FileStore {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get_credentials_sync(&self, provider: &str) -> Result<Option<Credentials>> {
        self.with_locked_store(false, |store| Ok(store.providers.get(provider).cloned()))
            .map(|credential| {
                credential.map(|credential| Credentials {
                    provider: provider.to_owned(),
                    kind: credential.kind,
                    secret: credential.secret,
                    refresh_secret: credential.refresh_secret,
                    metadata: credential.metadata,
                })
            })
    }

    pub fn store_credentials_sync(&self, provider: &str, credentials: Credentials) -> Result<()> {
        self.store_entry_sync(provider, StoredCredential::from(credentials))
    }

    pub(crate) fn store_credential_input_sync(
        &self,
        provider: &str,
        input: CredentialInput,
    ) -> Result<()> {
        self.store_entry_sync(provider, StoredCredential::from_input(input))
    }

    pub fn clear_credentials_sync(&self, provider: &str) -> Result<bool> {
        self.with_locked_store(true, |store| Ok(store.providers.remove(provider).is_some()))
    }

    pub fn detect_credentials_sync(&self, provider: &str) -> Result<DetectionResult> {
        let found =
            self.with_locked_store(false, |store| Ok(store.providers.contains_key(provider)))?;
        let sources = if found {
            vec![self.label()]
        } else {
            Vec::new()
        };
        Ok(DetectionResult {
            provider: provider.to_owned(),
            found,
            sources,
        })
    }

    pub(crate) fn with_locked_store<T>(
        &self,
        write_back: bool,
        mutator: impl FnOnce(&mut AuthStoreFile) -> Result<T>,
    ) -> Result<T> {
        if !write_back && !self.path.exists() {
            let mut store = AuthStoreFile::default();
            return mutator(&mut store);
        }

        let mut file = open_store_file(&self.path, write_back)?;
        if write_back {
            file.lock_exclusive()
                .map_err(|error| BelltowerError::Auth(error.to_string()))?;
        } else {
            file.lock_shared()
                .map_err(|error| BelltowerError::Auth(error.to_string()))?;
        }

        let result = (|| {
            let mut raw = String::new();
            file.seek(SeekFrom::Start(0))?;
            file.read_to_string(&mut raw)?;
            let mut store = parse_store(raw.trim())?;
            let result = mutator(&mut store)?;
            if write_back {
                file.set_len(0)?;
                file.seek(SeekFrom::Start(0))?;
                serde_json::to_writer_pretty(&mut file, &store)
                    .map_err(|error| BelltowerError::Auth(error.to_string()))?;
                file.write_all(b"\n")?;
                file.sync_all()?;
                set_owner_only_permissions(&file)?;
            }
            Ok(result)
        })();

        let unlock_result = file
            .unlock()
            .map_err(|error| BelltowerError::Auth(error.to_string()));
        match (result, unlock_result) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    fn store_entry_sync(&self, provider: &str, credential: StoredCredential) -> Result<()> {
        self.with_locked_store(true, |store| {
            store.providers.insert(provider.to_owned(), credential);
            Ok(())
        })
    }
}

impl CredentialStore for FileStore {
    fn load(&self, connection_id: &str) -> Result<Option<ResolvedCredential>> {
        let Some(stored) = self.with_locked_store(false, |store| {
            Ok(store.providers.get(connection_id).cloned())
        })?
        else {
            return Ok(None);
        };

        let Some(secret) = stored.resolve_secret()? else {
            return Ok(None);
        };

        Ok(Some(ResolvedCredential {
            provider: connection_id.to_owned(),
            kind: stored.kind,
            source: format!(
                "auth store ({}, {})",
                self.path.display(),
                describe_stored_secret_source(stored.source.as_ref(), &stored.secret)
            ),
            tried_sources: Vec::new(),
            secret,
            refresh_secret: stored.refresh_secret,
            metadata: stored.metadata,
        }))
    }

    fn store(&self, connection_id: &str, credentials: Credentials) -> Result<()> {
        self.store_credentials_sync(connection_id, credentials)
    }

    fn delete(&self, connection_id: &str) -> Result<bool> {
        self.clear_credentials_sync(connection_id)
    }

    fn list(&self) -> Result<Vec<String>> {
        self.with_locked_store(false, |store| Ok(store.providers.keys().cloned().collect()))
    }

    fn label(&self) -> String {
        format!("auth store ({})", self.path.display())
    }

    fn location(&self) -> Option<PathBuf> {
        Some(self.path.clone())
    }
}

impl AuthStore for FileStore {
    fn get_credentials(
        &self,
        provider: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<Credentials>>> + Send + '_>>
    {
        let provider = provider.to_owned();
        Box::pin(async move { self.get_credentials_sync(&provider) })
    }

    fn store_credentials(
        &self,
        provider: &str,
        credentials: Credentials,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>> {
        let provider = provider.to_owned();
        Box::pin(async move { self.store_credentials_sync(&provider, credentials) })
    }

    fn clear_credentials(
        &self,
        provider: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>> {
        let provider = provider.to_owned();
        Box::pin(async move {
            self.clear_credentials_sync(&provider)?;
            Ok(())
        })
    }

    fn detect_credentials(
        &self,
        provider: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<DetectionResult>> + Send + '_>>
    {
        let provider = provider.to_owned();
        Box::pin(async move { self.detect_credentials_sync(&provider) })
    }
}

fn open_store_file(path: &Path, write_back: bool) -> Result<File> {
    if write_back {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        set_owner_only_permissions(&file)?;
        return Ok(file);
    }

    OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(BelltowerError::Io)
}

fn set_owner_only_permissions(file: &File) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = file.metadata()?;
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        file.set_permissions(permissions)?;
    }
    Ok(())
}

fn parse_store(raw: &str) -> Result<AuthStoreFile> {
    if raw.is_empty() {
        return Ok(AuthStoreFile::default());
    }
    serde_json::from_str(raw).map_err(|error| BelltowerError::Auth(error.to_string()))
}
