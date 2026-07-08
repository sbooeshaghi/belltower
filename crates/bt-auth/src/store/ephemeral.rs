use super::{CredentialStore, empty_to_none, oauth_metadata_from_parts, resolved_from_credentials};
use bt_core::{BelltowerError, CredentialKind, Credentials, Result};
use std::collections::BTreeSet;
use std::env;
use std::path::PathBuf;

const ENV_PREFIX: &str = "BELLTOWER_";
const API_KEY_SUFFIX: &str = "_API_KEY";
const ACCESS_TOKEN_SUFFIX: &str = "_ACCESS_TOKEN";
const REFRESH_TOKEN_SUFFIX: &str = "_REFRESH_TOKEN";
const JSON_DOCUMENT_SUFFIX: &str = "_JSON_DOCUMENT";
const ACCOUNT_ID_SUFFIX: &str = "_ACCOUNT_ID";
const PLAN_TYPE_SUFFIX: &str = "_PLAN_TYPE";
const WORKSPACE_ID_SUFFIX: &str = "_WORKSPACE_ID";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EphemeralEnvNames {
    pub api_key: String,
    pub access_token: String,
    pub refresh_token: String,
    pub json_document: String,
    pub account_id: String,
    pub plan_type: String,
    pub workspace_id: String,
}

#[must_use]
pub fn ephemeral_env_names(connection_id: &str) -> EphemeralEnvNames {
    let encoded = encode_connection_id(connection_id);
    EphemeralEnvNames {
        api_key: format!("{ENV_PREFIX}{encoded}{API_KEY_SUFFIX}"),
        access_token: format!("{ENV_PREFIX}{encoded}{ACCESS_TOKEN_SUFFIX}"),
        refresh_token: format!("{ENV_PREFIX}{encoded}{REFRESH_TOKEN_SUFFIX}"),
        json_document: format!("{ENV_PREFIX}{encoded}{JSON_DOCUMENT_SUFFIX}"),
        account_id: format!("{ENV_PREFIX}{encoded}{ACCOUNT_ID_SUFFIX}"),
        plan_type: format!("{ENV_PREFIX}{encoded}{PLAN_TYPE_SUFFIX}"),
        workspace_id: format!("{ENV_PREFIX}{encoded}{WORKSPACE_ID_SUFFIX}"),
    }
}

#[derive(Clone, Debug, Default)]
pub struct EphemeralStore;

impl CredentialStore for EphemeralStore {
    fn load(&self, connection_id: &str) -> Result<Option<crate::ResolvedCredential>> {
        let names = ephemeral_env_names(connection_id);

        if let Some(access_token) = read_env(&names.access_token)? {
            let credentials = Credentials {
                provider: connection_id.to_owned(),
                kind: CredentialKind::OAuthToken,
                secret: access_token,
                refresh_secret: read_env(&names.refresh_token)?,
                metadata: oauth_metadata_from_parts(
                    read_env(&names.account_id)?,
                    read_env(&names.plan_type)?,
                    read_env(&names.workspace_id)?,
                ),
            };
            return Ok(Some(resolved_from_credentials(
                connection_id,
                format!("ephemeral env:{}", names.access_token),
                credentials,
            )));
        }

        if let Some(document) = read_env(&names.json_document)? {
            let credentials = Credentials {
                provider: connection_id.to_owned(),
                kind: CredentialKind::JsonDocument,
                secret: document,
                refresh_secret: None,
                metadata: Default::default(),
            };
            return Ok(Some(resolved_from_credentials(
                connection_id,
                format!("ephemeral env:{}", names.json_document),
                credentials,
            )));
        }

        let Some(secret) = read_env(&names.api_key)? else {
            return Ok(None);
        };
        let credentials = Credentials {
            provider: connection_id.to_owned(),
            kind: CredentialKind::ApiKey,
            secret,
            refresh_secret: None,
            metadata: Default::default(),
        };
        Ok(Some(resolved_from_credentials(
            connection_id,
            format!("ephemeral env:{}", names.api_key),
            credentials,
        )))
    }

    fn store(&self, connection_id: &str, _credentials: Credentials) -> Result<()> {
        Err(BelltowerError::Auth(format!(
            "ephemeral credential store does not persist `{connection_id}`; use environment variables instead"
        )))
    }

    fn delete(&self, connection_id: &str) -> Result<bool> {
        Err(BelltowerError::Auth(format!(
            "ephemeral credential store cannot delete `{connection_id}`; unset the matching BELLTOWER_* environment variables instead"
        )))
    }

    fn list(&self) -> Result<Vec<String>> {
        let mut connections = BTreeSet::new();
        for (key, value) in env::vars() {
            if value.trim().is_empty() {
                continue;
            }
            if let Some(connection_id) = connection_id_from_env_key(&key) {
                connections.insert(connection_id);
            }
        }
        Ok(connections.into_iter().collect())
    }

    fn label(&self) -> String {
        "ephemeral environment".to_owned()
    }

    fn location(&self) -> Option<PathBuf> {
        None
    }
}

fn read_env(key: &str) -> Result<Option<String>> {
    match env::var(key) {
        Ok(value) => Ok(empty_to_none(Some(value))),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(BelltowerError::Auth(error.to_string())),
    }
}

fn encode_connection_id(connection_id: &str) -> String {
    connection_id.replace('-', "__").to_ascii_uppercase()
}

fn decode_connection_id(encoded: &str) -> String {
    encoded.replace("__", "-").to_ascii_lowercase()
}

fn connection_id_from_env_key(key: &str) -> Option<String> {
    let encoded = [
        API_KEY_SUFFIX,
        ACCESS_TOKEN_SUFFIX,
        REFRESH_TOKEN_SUFFIX,
        JSON_DOCUMENT_SUFFIX,
        ACCOUNT_ID_SUFFIX,
        PLAN_TYPE_SUFFIX,
        WORKSPACE_ID_SUFFIX,
    ]
    .iter()
    .find_map(|suffix| {
        key.strip_prefix(ENV_PREFIX)
            .and_then(|rest| rest.strip_suffix(suffix))
    })?;

    (!encoded.is_empty()).then(|| decode_connection_id(encoded))
}
