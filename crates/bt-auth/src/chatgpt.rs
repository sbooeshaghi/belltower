use crate::{CredentialStore, FileAuthStore};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use bt_core::{BelltowerError, CredentialKind, CredentialMetadata, Credentials, Result};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize, de::Deserializer};
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const OPENAI_CHATGPT_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const OPENAI_CHATGPT_ISSUER: &str = "https://auth.openai.com";

const DEFAULT_DEVICE_AUTH_POLL_INTERVAL: u64 = 5;
const DEFAULT_DEVICE_AUTH_TIMEOUT_SECS: u64 = 15 * 60;

#[derive(Clone)]
pub struct ChatGptLoginOptions {
    provider: String,
    issuer: String,
    client_id: String,
    store: Arc<dyn CredentialStore>,
    timeout: Duration,
}

impl ChatGptLoginOptions {
    #[must_use]
    pub fn for_provider(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            issuer: OPENAI_CHATGPT_ISSUER.to_owned(),
            client_id: OPENAI_CHATGPT_CLIENT_ID.to_owned(),
            store: Arc::new(FileAuthStore::default()),
            timeout: Duration::from_secs(DEFAULT_DEVICE_AUTH_TIMEOUT_SECS),
        }
    }

    #[must_use]
    pub fn with_issuer(mut self, issuer: impl Into<String>) -> Self {
        self.issuer = issuer.into().trim_end_matches('/').to_owned();
        self
    }

    #[must_use]
    pub fn with_client_id(mut self, client_id: impl Into<String>) -> Self {
        self.client_id = client_id.into();
        self
    }

    #[must_use]
    pub fn with_store(mut self, store: FileAuthStore) -> Self {
        self.store = Arc::new(store);
        self
    }

    #[must_use]
    pub fn with_shared_store(mut self, store: Arc<dyn CredentialStore>) -> Self {
        self.store = store;
        self
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn provider(&self) -> &str {
        &self.provider
    }

    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    #[must_use]
    pub fn client_id(&self) -> &str {
        &self.client_id
    }
}

impl std::fmt::Debug for ChatGptLoginOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChatGptLoginOptions")
            .field("provider", &self.provider)
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("store", &self.store.label())
            .field("timeout", &self.timeout)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct ChatGptDeviceCodeSession {
    client: reqwest::Client,
    options: ChatGptLoginOptions,
    device_auth_id: String,
    user_code: String,
    interval: Duration,
}

impl ChatGptDeviceCodeSession {
    pub async fn start(options: ChatGptLoginOptions) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent("belltower/0.1.0")
            .build()
            .map_err(reqwest_error)?;
        let request = DeviceCodeStartRequest {
            client_id: options.client_id.clone(),
        };
        let endpoint = format!("{}/api/accounts/deviceauth/usercode", options.issuer);
        let response = client
            .post(endpoint)
            .json(&request)
            .send()
            .await
            .map_err(reqwest_error)?;
        let response = expect_success(response, "start ChatGPT device login").await?;
        let started: DeviceCodeStartResponse = response.json().await.map_err(reqwest_error)?;
        Ok(Self {
            client,
            options,
            device_auth_id: started.device_auth_id,
            user_code: started.user_code,
            interval: Duration::from_secs(started.interval.max(1)),
        })
    }

    #[must_use]
    pub fn authorize_url(&self) -> String {
        format!("{}/api/accounts/deviceauth/authorize", self.options.issuer)
    }

    #[must_use]
    pub fn user_code(&self) -> &str {
        &self.user_code
    }

    pub async fn wait_for_completion(self) -> Result<String> {
        let deadline = Instant::now() + self.options.timeout;
        loop {
            let poll = self.poll_once().await?;
            match poll {
                DeviceCodePollResult::Pending => {
                    if Instant::now() >= deadline {
                        return Err(BelltowerError::Auth(
                            "timed out waiting for ChatGPT device authorization".to_owned(),
                        ));
                    }
                    tokio::time::sleep(self.interval).await;
                }
                DeviceCodePollResult::Authorized(authorized) => {
                    let tokens = self
                        .exchange_authorization_code(
                            authorized.authorization_code,
                            authorized.code_verifier,
                        )
                        .await?;
                    return self.store_token_bundle(
                        tokens.access_token,
                        tokens.refresh_token,
                        &tokens.id_token,
                    );
                }
            }
        }
    }

    async fn poll_once(&self) -> Result<DeviceCodePollResult> {
        let endpoint = format!("{}/api/accounts/deviceauth/token", self.options.issuer);
        let response = self
            .client
            .post(endpoint)
            .json(&DeviceCodePollRequest {
                device_auth_id: self.device_auth_id.clone(),
                user_code: self.user_code.clone(),
            })
            .send()
            .await
            .map_err(reqwest_error)?;

        match response.status() {
            StatusCode::OK => {
                let payload: DeviceCodeAuthorizedResponse =
                    response.json().await.map_err(reqwest_error)?;
                Ok(DeviceCodePollResult::Authorized(payload))
            }
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => Ok(DeviceCodePollResult::Pending),
            status => {
                let body = response.text().await.map_err(reqwest_error)?;
                Err(BelltowerError::Auth(format!(
                    "ChatGPT device authorization failed with status {status}: {}",
                    compact_body(&body)
                )))
            }
        }
    }

    async fn exchange_authorization_code(
        &self,
        authorization_code: String,
        code_verifier: Option<String>,
    ) -> Result<TokenExchangeResponse> {
        let endpoint = format!("{}/oauth/token", self.options.issuer);
        let redirect_uri = format!("{}/deviceauth/callback", self.options.issuer);
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("code", authorization_code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", self.options.client_id.as_str()),
        ];
        if let Some(code_verifier) = code_verifier.as_deref() {
            form.push(("code_verifier", code_verifier));
        }
        let response = self
            .client
            .post(endpoint)
            .form(&form)
            .send()
            .await
            .map_err(reqwest_error)?;
        let response = expect_success(response, "exchange ChatGPT authorization code").await?;
        response.json().await.map_err(reqwest_error)
    }

    fn store_token_bundle(
        &self,
        access_token: String,
        refresh_token: Option<String>,
        id_token: &str,
    ) -> Result<String> {
        let metadata = metadata_from_tokens(id_token, &access_token)?;
        self.options.store.store(
            self.options.provider(),
            Credentials {
                provider: self.options.provider().to_owned(),
                kind: CredentialKind::OAuthToken,
                secret: access_token,
                refresh_secret: refresh_token,
                metadata,
            },
        )?;
        Ok(self.options.store.label())
    }
}

pub async fn refresh_chatgpt_token_bundle(options: ChatGptLoginOptions) -> Result<String> {
    let Some(credentials) = options.store.load(options.provider())? else {
        return Err(BelltowerError::Auth(format!(
            "no stored ChatGPT credentials found for `{}`",
            options.provider()
        )));
    };
    let Some(refresh_token) = credentials.refresh_secret.clone() else {
        return Err(BelltowerError::Auth(format!(
            "stored ChatGPT credentials for `{}` do not include a refresh token",
            options.provider()
        )));
    };

    let client = reqwest::Client::builder()
        .user_agent("belltower/0.1.0")
        .build()
        .map_err(reqwest_error)?;
    let response = client
        .post(format!("{}/oauth/token", options.issuer))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("client_id", options.client_id.as_str()),
        ])
        .send()
        .await
        .map_err(reqwest_error)?;
    let response = expect_success(response, "refresh ChatGPT access token").await?;
    let refreshed: TokenExchangeResponse = response.json().await.map_err(reqwest_error)?;
    let metadata = metadata_from_tokens(&refreshed.id_token, &refreshed.access_token)?;
    let refresh_secret = refreshed.refresh_token.or(credentials.refresh_secret);
    options.store.store(
        options.provider(),
        Credentials {
            provider: options.provider().to_owned(),
            kind: CredentialKind::OAuthToken,
            secret: refreshed.access_token,
            refresh_secret,
            metadata,
        },
    )?;
    Ok(options.store.label())
}

#[cfg(test)]
pub(crate) fn access_token_expires_within(access_token: &str, within: Duration) -> Result<bool> {
    let claims = decode_jwt_claims(access_token)?;
    let Some(expiration_epoch) = expiration_epoch_seconds(&claims) else {
        return Ok(false);
    };
    Ok(expiration_epoch_within(expiration_epoch, within))
}

pub(crate) fn access_token_refresh_required(access_token: &str, within: Duration) -> Result<bool> {
    let claims = decode_jwt_claims(access_token)?;
    let Some(expiration_epoch) = expiration_epoch_seconds(&claims) else {
        return Ok(true);
    };
    Ok(expiration_epoch_within(expiration_epoch, within))
}

fn expiration_epoch_within(expiration_epoch: u64, within: Duration) -> bool {
    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    expiration_epoch <= now_epoch.saturating_add(within.as_secs())
}

#[derive(Debug)]
enum DeviceCodePollResult {
    Pending,
    Authorized(DeviceCodeAuthorizedResponse),
}

#[derive(Debug, Serialize)]
struct DeviceCodeStartRequest {
    client_id: String,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeStartResponse {
    device_auth_id: String,
    user_code: String,
    #[serde(
        default = "default_device_auth_poll_interval",
        deserialize_with = "deserialize_interval"
    )]
    interval: u64,
}

#[derive(Debug, Serialize)]
struct DeviceCodePollRequest {
    device_auth_id: String,
    user_code: String,
}

#[derive(Clone, Debug, Deserialize)]
struct DeviceCodeAuthorizedResponse {
    authorization_code: String,
    #[serde(default)]
    code_verifier: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct TokenExchangeResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    id_token: String,
}

fn default_device_auth_poll_interval() -> u64 {
    DEFAULT_DEVICE_AUTH_POLL_INTERVAL
}

fn deserialize_interval<'de, D>(deserializer: D) -> std::result::Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum IntervalValue {
        Integer(u64),
        Text(String),
    }

    let value = Option::<IntervalValue>::deserialize(deserializer)?;
    match value {
        None => Ok(default_device_auth_poll_interval()),
        Some(IntervalValue::Integer(value)) => Ok(value),
        Some(IntervalValue::Text(value)) => value.parse::<u64>().map_err(serde::de::Error::custom),
    }
}

async fn expect_success(response: reqwest::Response, operation: &str) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.map_err(reqwest_error)?;
    Err(BelltowerError::Auth(format!(
        "failed to {operation}: status {status}: {}",
        compact_body(&body)
    )))
}

fn compact_body(body: &str) -> String {
    let compact = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        "empty response".to_owned()
    } else {
        compact
    }
}

fn metadata_from_tokens(id_token: &str, access_token: &str) -> Result<CredentialMetadata> {
    let id_claims = decode_jwt_claims(id_token)?;
    let access_claims = decode_jwt_claims(access_token)?;

    Ok(CredentialMetadata {
        account_id: top_level_or_nested_claim(&id_claims, "chatgpt_account_id")
            .or_else(|| top_level_or_nested_claim(&access_claims, "chatgpt_account_id")),
        plan_type: top_level_or_nested_claim(&id_claims, "chatgpt_plan_type")
            .or_else(|| top_level_or_nested_claim(&access_claims, "chatgpt_plan_type")),
        workspace_id: top_level_or_nested_claim(&id_claims, "workspace_id")
            .or_else(|| top_level_or_nested_claim(&access_claims, "workspace_id")),
    })
}

fn decode_jwt_claims(token: &str) -> Result<Value> {
    let mut segments = token.split('.');
    let _header = segments.next();
    let payload = segments.next().ok_or_else(|| {
        BelltowerError::Auth("ChatGPT token did not contain a JWT payload".to_owned())
    })?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).map_err(|error| {
        BelltowerError::Auth(format!("failed to decode ChatGPT token payload: {error}"))
    })?;
    serde_json::from_slice(&decoded).map_err(|error| {
        BelltowerError::Auth(format!("failed to parse ChatGPT token payload: {error}"))
    })
}

fn top_level_or_nested_claim(claims: &Value, key: &str) -> Option<String> {
    claims
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            claims
                .get("https://api.openai.com/auth")
                .and_then(|value| value.get(key))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
}

fn expiration_epoch_seconds(claims: &Value) -> Option<u64> {
    match claims.get("exp") {
        Some(Value::Number(value)) => value.as_u64().or_else(|| {
            value
                .as_i64()
                .filter(|epoch| *epoch >= 0)
                .map(|epoch| epoch as u64)
        }),
        Some(Value::String(value)) => value.parse::<u64>().ok(),
        _ => None,
    }
}

fn reqwest_error(error: reqwest::Error) -> BelltowerError {
    BelltowerError::Auth(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        ChatGptDeviceCodeSession, ChatGptLoginOptions, access_token_expires_within,
        access_token_refresh_required, refresh_chatgpt_token_bundle,
    };
    use crate::{CredentialStore, FileAuthStore};
    use axum::{Json, Router, extract::State, routing::post};
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use bt_core::{CredentialKind, CredentialMetadata, Credentials, Result};
    use serde_json::{Value, json};
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tempfile::TempDir;

    #[derive(Clone)]
    struct TestState {
        refresh_requests: Arc<Mutex<Vec<String>>>,
        rotate_refresh_token: bool,
    }

    #[tokio::test]
    async fn device_code_flow_stores_refreshable_oauth_bundle() -> Result<()> {
        let (_temp, store) = temp_store();
        let state = TestState {
            refresh_requests: Arc::new(Mutex::new(Vec::new())),
            rotate_refresh_token: true,
        };
        let base_url = serve_test_issuer(state).await?;
        let options = ChatGptLoginOptions::for_provider("chatgpt")
            .with_store(store.clone())
            .with_issuer(base_url.clone())
            .with_client_id("chatgpt-test-client")
            .with_timeout(Duration::from_secs(1));

        let session = ChatGptDeviceCodeSession::start(options).await?;
        assert_eq!(
            session.authorize_url(),
            format!("{base_url}/api/accounts/deviceauth/authorize")
        );
        assert_eq!(session.user_code(), "ABCD-EFGH");

        let location = session.wait_for_completion().await?;
        assert_eq!(location, store.label());

        let stored = store
            .get_credentials_sync("chatgpt")?
            .expect("credential should be stored");
        assert_eq!(stored.kind, CredentialKind::OAuthToken);
        assert_eq!(stored.refresh_secret.as_deref(), Some("refresh-token-1"));
        assert_eq!(
            stored.metadata,
            CredentialMetadata {
                account_id: Some("acct_device".to_owned()),
                plan_type: Some("plus".to_owned()),
                workspace_id: Some("workspace_device".to_owned()),
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn refresh_flow_replaces_access_and_refresh_tokens() -> Result<()> {
        let (_temp, store) = temp_store();
        store.store_credentials_sync(
            "chatgpt",
            Credentials {
                provider: "chatgpt".to_owned(),
                kind: CredentialKind::OAuthToken,
                secret: "stale-access".to_owned(),
                refresh_secret: Some("refresh-token-0".to_owned()),
                metadata: CredentialMetadata::default(),
            },
        )?;
        let refresh_requests = Arc::new(Mutex::new(Vec::new()));
        let base_url = serve_test_issuer(TestState {
            refresh_requests: refresh_requests.clone(),
            rotate_refresh_token: true,
        })
        .await?;

        let location = refresh_chatgpt_token_bundle(
            ChatGptLoginOptions::for_provider("chatgpt")
                .with_store(store.clone())
                .with_issuer(base_url)
                .with_client_id("chatgpt-test-client"),
        )
        .await?;
        assert_eq!(location, store.label());

        let stored = store
            .get_credentials_sync("chatgpt")?
            .expect("credential should be stored");
        assert_ne!(stored.secret, "stale-access");
        assert_eq!(stored.refresh_secret.as_deref(), Some("refresh-token-2"));
        assert_eq!(stored.metadata.plan_type.as_deref(), Some("pro"));
        assert_eq!(
            refresh_requests
                .lock()
                .expect("refresh requests")
                .as_slice(),
            &["refresh-token-0".to_owned()]
        );
        Ok(())
    }

    #[tokio::test]
    async fn refresh_flow_preserves_existing_refresh_token_when_response_omits_one() -> Result<()> {
        let (_temp, store) = temp_store();
        store.store_credentials_sync(
            "chatgpt",
            Credentials {
                provider: "chatgpt".to_owned(),
                kind: CredentialKind::OAuthToken,
                secret: "stale-access".to_owned(),
                refresh_secret: Some("refresh-token-0".to_owned()),
                metadata: CredentialMetadata::default(),
            },
        )?;
        let refresh_requests = Arc::new(Mutex::new(Vec::new()));
        let base_url = serve_test_issuer(TestState {
            refresh_requests: refresh_requests.clone(),
            rotate_refresh_token: false,
        })
        .await?;

        refresh_chatgpt_token_bundle(
            ChatGptLoginOptions::for_provider("chatgpt")
                .with_store(store.clone())
                .with_issuer(base_url)
                .with_client_id("chatgpt-test-client"),
        )
        .await?;

        let stored = store
            .get_credentials_sync("chatgpt")?
            .expect("credential should be stored");
        assert_ne!(stored.secret, "stale-access");
        assert_eq!(stored.refresh_secret.as_deref(), Some("refresh-token-0"));
        assert_eq!(
            refresh_requests
                .lock()
                .expect("refresh requests")
                .as_slice(),
            &["refresh-token-0".to_owned()]
        );
        Ok(())
    }

    async fn serve_test_issuer(state: TestState) -> Result<String> {
        async fn start_device_auth(Json(payload): Json<Value>) -> Json<Value> {
            assert_eq!(
                payload.get("client_id").and_then(Value::as_str),
                Some("chatgpt-test-client")
            );
            Json(json!({
                "device_auth_id": "device-auth-1",
                "user_code": "ABCD-EFGH",
                "interval": "0"
            }))
        }

        async fn poll_device_auth(Json(payload): Json<Value>) -> Json<Value> {
            assert_eq!(
                payload.get("device_auth_id").and_then(Value::as_str),
                Some("device-auth-1")
            );
            assert_eq!(
                payload.get("user_code").and_then(Value::as_str),
                Some("ABCD-EFGH")
            );
            Json(json!({
                "authorization_code": "device-auth-code-1"
            }))
        }

        async fn exchange_tokens(
            State(state): State<TestState>,
            form: axum::extract::Form<std::collections::HashMap<String, String>>,
        ) -> Json<Value> {
            match form.0.get("grant_type").map(String::as_str) {
                Some("authorization_code") => Json(json!({
                    "access_token": fake_jwt(json!({
                        "chatgpt_account_id": "acct_device",
                        "https://api.openai.com/auth": {
                            "chatgpt_plan_type": "plus"
                        }
                    })),
                    "refresh_token": "refresh-token-1",
                    "id_token": fake_jwt(json!({
                        "chatgpt_account_id": "acct_device",
                        "workspace_id": "workspace_device",
                        "https://api.openai.com/auth": {
                            "chatgpt_plan_type": "plus"
                        }
                    }))
                })),
                Some("refresh_token") => {
                    if let Some(value) = form.0.get("refresh_token") {
                        state
                            .refresh_requests
                            .lock()
                            .expect("refresh requests lock")
                            .push(value.clone());
                    }
                    let mut response = json!({
                        "access_token": fake_jwt(json!({
                            "chatgpt_account_id": "acct_refresh",
                            "https://api.openai.com/auth": {
                                "chatgpt_plan_type": "pro"
                            }
                        })),
                        "id_token": fake_jwt(json!({
                            "chatgpt_account_id": "acct_refresh",
                            "workspace_id": "workspace_refresh",
                            "https://api.openai.com/auth": {
                                "chatgpt_plan_type": "pro"
                            }
                        }))
                    });
                    if state.rotate_refresh_token {
                        response["refresh_token"] = Value::String("refresh-token-2".to_owned());
                    }
                    Json(response)
                }
                other => panic!("unexpected grant type: {other:?}"),
            }
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address: SocketAddr = listener.local_addr()?;
        let app = Router::new()
            .route("/api/accounts/deviceauth/usercode", post(start_device_auth))
            .route("/api/accounts/deviceauth/token", post(poll_device_auth))
            .route("/oauth/token", post(exchange_tokens))
            .with_state(state);
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test issuer server");
        });
        Ok(format!("http://{address}"))
    }

    fn temp_store() -> (TempDir, FileAuthStore) {
        let temp = TempDir::new().expect("tempdir");
        let store = FileAuthStore::new(temp.path().join("auth.json"));
        (temp, store)
    }

    fn fake_jwt(claims: Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&claims).expect("serialize fake token claims"));
        format!("{header}.{payload}.signature")
    }

    #[test]
    fn access_token_expiry_detection_uses_exp_claim() -> Result<()> {
        let now_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let expiring = fake_jwt(json!({ "exp": now_epoch + 30 }));
        let fresh = fake_jwt(json!({ "exp": now_epoch + 3600 }));
        let no_exp = fake_jwt(json!({ "chatgpt_account_id": "acct" }));

        assert!(access_token_expires_within(
            &expiring,
            Duration::from_secs(60)
        )?);
        assert!(!access_token_expires_within(
            &fresh,
            Duration::from_secs(60)
        )?);
        assert!(!access_token_expires_within(
            &no_exp,
            Duration::from_secs(60)
        )?);
        Ok(())
    }

    #[test]
    fn access_token_refresh_requirement_is_conservative_without_exp() -> Result<()> {
        let now_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let expiring = fake_jwt(json!({ "exp": now_epoch + 30 }));
        let fresh = fake_jwt(json!({ "exp": now_epoch + 3600 }));
        let no_exp = fake_jwt(json!({ "chatgpt_account_id": "acct" }));

        assert!(access_token_refresh_required(
            &expiring,
            Duration::from_secs(60)
        )?);
        assert!(!access_token_refresh_required(
            &fresh,
            Duration::from_secs(60)
        )?);
        assert!(access_token_refresh_required(
            &no_exp,
            Duration::from_secs(60)
        )?);
        Ok(())
    }
}
