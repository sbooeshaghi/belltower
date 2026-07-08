use bt_auth::{
    AutoStore, CredentialStore, EphemeralStore, ResolvedCredential, ephemeral_env_names,
};
use bt_core::{BelltowerError, CredentialKind, CredentialMetadata, Credentials, Result};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[cfg(feature = "keychain-backend")]
use bt_auth::KeychainStore;

#[derive(Clone)]
enum StubLoad {
    Missing,
    Present(ResolvedCredential),
    Error(String),
}

#[derive(Clone)]
enum StubList {
    Ok(Vec<String>),
    Error(String),
}

#[derive(Clone)]
struct StubStore {
    label: String,
    load: StubLoad,
    list: StubList,
    writes: Option<Arc<Mutex<Vec<String>>>>,
}

impl StubStore {
    fn missing(label: &str) -> Self {
        Self {
            label: label.to_owned(),
            load: StubLoad::Missing,
            list: StubList::Ok(Vec::new()),
            writes: None,
        }
    }

    fn present(label: &str, source: &str) -> Self {
        Self {
            label: label.to_owned(),
            load: StubLoad::Present(ResolvedCredential {
                provider: "openai".to_owned(),
                kind: CredentialKind::ApiKey,
                source: source.to_owned(),
                tried_sources: Vec::new(),
                secret: "sk-test".to_owned(),
                refresh_secret: None,
                metadata: CredentialMetadata::default(),
            }),
            list: StubList::Ok(vec!["openai".to_owned()]),
            writes: None,
        }
    }

    #[cfg(feature = "keychain-backend")]
    fn unavailable(label: &str, reason: &str) -> Self {
        Self {
            label: label.to_owned(),
            load: StubLoad::Error(reason.to_owned()),
            list: StubList::Error(reason.to_owned()),
            writes: None,
        }
    }

    fn with_writes(mut self, writes: Arc<Mutex<Vec<String>>>) -> Self {
        self.writes = Some(writes);
        self
    }
}

impl CredentialStore for StubStore {
    fn load(&self, _connection_id: &str) -> Result<Option<ResolvedCredential>> {
        match &self.load {
            StubLoad::Missing => Ok(None),
            StubLoad::Present(credential) => Ok(Some(credential.clone())),
            StubLoad::Error(reason) => Err(BelltowerError::Auth(reason.clone())),
        }
    }

    fn store(&self, connection_id: &str, _credentials: Credentials) -> Result<()> {
        if let Some(writes) = &self.writes {
            writes
                .lock()
                .expect("stub store writes")
                .push(connection_id.to_owned());
        }
        Ok(())
    }

    fn delete(&self, _connection_id: &str) -> Result<bool> {
        Ok(false)
    }

    fn list(&self) -> Result<Vec<String>> {
        match &self.list {
            StubList::Ok(entries) => Ok(entries.clone()),
            StubList::Error(reason) => Err(BelltowerError::Auth(reason.clone())),
        }
    }

    fn label(&self) -> String {
        self.label.clone()
    }

    fn location(&self) -> Option<PathBuf> {
        None
    }
}

#[test]
fn ephemeral_store_reads_env_backed_oauth_and_fails_closed_on_mutation() {
    let store = EphemeralStore;
    let openai = ephemeral_env_names("openai");
    let chatgpt = ephemeral_env_names("openai-chatgpt");

    temp_env::with_vars(
        [
            (openai.api_key.as_str(), Some("sk-openai")),
            (chatgpt.access_token.as_str(), Some("access-token")),
            (chatgpt.refresh_token.as_str(), Some("refresh-token")),
            (chatgpt.account_id.as_str(), Some("acct-123")),
            (chatgpt.plan_type.as_str(), Some("pro")),
            (chatgpt.workspace_id.as_str(), Some("ws-main")),
        ],
        || {
            let api_key = store
                .load("openai")
                .expect("load openai")
                .expect("openai credential");
            assert_eq!(api_key.kind, CredentialKind::ApiKey);
            assert_eq!(api_key.secret, "sk-openai");
            assert!(api_key.source.contains(&openai.api_key));

            let oauth = store
                .load("openai-chatgpt")
                .expect("load chatgpt")
                .expect("chatgpt credential");
            assert_eq!(oauth.kind, CredentialKind::OAuthToken);
            assert_eq!(oauth.secret, "access-token");
            assert_eq!(oauth.refresh_secret.as_deref(), Some("refresh-token"));
            assert_eq!(oauth.metadata.account_id.as_deref(), Some("acct-123"));
            assert_eq!(oauth.metadata.plan_type.as_deref(), Some("pro"));
            assert_eq!(oauth.metadata.workspace_id.as_deref(), Some("ws-main"));

            let listed = store.list().expect("list ephemeral credentials");
            assert_eq!(
                listed,
                vec!["openai".to_owned(), "openai-chatgpt".to_owned()]
            );

            let error = store
                .store(
                    "openai",
                    Credentials {
                        provider: "openai".to_owned(),
                        kind: CredentialKind::ApiKey,
                        secret: "ignored".to_owned(),
                        refresh_secret: None,
                        metadata: Default::default(),
                    },
                )
                .expect_err("ephemeral store must reject writes");
            assert!(error.to_string().contains("does not persist"));

            let error = store
                .delete("openai")
                .expect_err("ephemeral store must reject deletes");
            assert!(error.to_string().contains("cannot delete"));
        },
    );
}

#[cfg(feature = "keychain-backend")]
#[test]
fn auto_store_prefers_ephemeral_over_keychain_and_file() {
    let store = AutoStore::with_backends(
        Arc::new(StubStore::present(
            "ephemeral environment",
            "ephemeral env:BELLTOWER_OPENAI_API_KEY",
        )),
        Arc::new(StubStore::present(
            "keychain (service belltower)",
            "keychain (service belltower, account openai)",
        )),
        Arc::new(StubStore::present(
            "auth store (/tmp/auth.json)",
            "auth store (/tmp/auth.json, literal)",
        )),
    );

    let resolved = store.load("openai").expect("resolve").expect("credential");
    assert_eq!(resolved.source, "ephemeral env:BELLTOWER_OPENAI_API_KEY");
    assert_eq!(
        resolved.tried_sources,
        vec!["ephemeral environment".to_owned()]
    );
}

#[cfg(feature = "keychain-backend")]
#[test]
fn auto_store_falls_through_when_keychain_is_unavailable() {
    let store = AutoStore::with_backends(
        Arc::new(StubStore::missing("ephemeral environment")),
        Arc::new(StubStore::unavailable(
            "keychain (service belltower)",
            "keychain unavailable",
        )),
        Arc::new(StubStore::present(
            "auth store (/tmp/auth.json)",
            "auth store (/tmp/auth.json, literal)",
        )),
    );

    let resolved = store.load("openai").expect("resolve").expect("credential");
    assert_eq!(resolved.source, "auth store (/tmp/auth.json, literal)");
    assert_eq!(resolved.tried_sources[0], "ephemeral environment");
    let tried = resolved.tried_sources.join(" | ");
    assert!(tried.contains("keychain (service belltower)"));
    assert!(
        tried.contains("unavailable"),
        "expected unavailable keychain in tried trail: {tried}"
    );
    assert!(tried.contains("keychain unavailable"));
    assert!(
        resolved
            .tried_sources
            .iter()
            .any(|entry| entry == "auth store (/tmp/auth.json)")
    );
}

#[cfg(feature = "keychain-backend")]
#[test]
fn auto_store_updates_existing_file_credential_before_keychain_default_write() {
    let keychain_writes = Arc::new(Mutex::new(Vec::new()));
    let file_writes = Arc::new(Mutex::new(Vec::new()));
    let store = AutoStore::with_backends(
        Arc::new(StubStore::missing("ephemeral environment")),
        Arc::new(
            StubStore::missing("keychain (service belltower)").with_writes(keychain_writes.clone()),
        ),
        Arc::new(
            StubStore::present(
                "auth store (/tmp/auth.json)",
                "auth store (/tmp/auth.json, literal)",
            )
            .with_writes(file_writes.clone()),
        ),
    );

    store
        .store(
            "openai",
            Credentials {
                provider: "openai".to_owned(),
                kind: CredentialKind::ApiKey,
                secret: "sk-rotated".to_owned(),
                refresh_secret: None,
                metadata: CredentialMetadata::default(),
            },
        )
        .expect("store updated credential");

    assert!(
        keychain_writes.lock().expect("keychain writes").is_empty(),
        "existing file credentials must be updated in place instead of being copied to keychain"
    );
    assert_eq!(
        file_writes.lock().expect("file writes").as_slice(),
        &["openai".to_owned()]
    );
}

#[cfg(feature = "keychain-backend")]
#[test]
fn keychain_store_round_trips_credentials_when_enabled() {
    if std::env::var("BELLTOWER_RUN_KEYCHAIN_TESTS").as_deref() != Ok("1") {
        return;
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let service = format!("belltower.test.{nanos}");
    let connection_id = format!("openai-{nanos}");
    let store = KeychainStore::new(service.clone());

    let _ = store.delete(&connection_id);
    let credentials = Credentials {
        provider: connection_id.clone(),
        kind: CredentialKind::ApiKey,
        secret: "sk-keychain".to_owned(),
        refresh_secret: None,
        metadata: Default::default(),
    };
    store
        .store(&connection_id, credentials.clone())
        .expect("store keychain credential");

    let loaded = store
        .load(&connection_id)
        .expect("load keychain credential")
        .expect("stored credential");
    assert_eq!(loaded.provider, connection_id);
    assert_eq!(loaded.kind, credentials.kind);
    assert_eq!(loaded.secret, credentials.secret);
    assert!(loaded.source.contains(&service));

    let listed = store.list().expect("list keychain credentials");
    assert_eq!(listed, vec![connection_id.clone()]);

    assert!(
        store
            .delete(&connection_id)
            .expect("delete keychain credential")
    );
    assert!(
        store
            .load(&connection_id)
            .expect("load after delete")
            .is_none()
    );
}
