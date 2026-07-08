use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    Config,
    Auth,
    Provider,
    Tool,
    Protocol,
    Storage,
    NotFound,
    #[default]
    Runtime,
    Unsupported,
}

impl ErrorClass {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Auth => "auth",
            Self::Provider => "provider",
            Self::Tool => "tool",
            Self::Protocol => "protocol",
            Self::Storage => "storage",
            Self::NotFound => "not_found",
            Self::Runtime => "runtime",
            Self::Unsupported => "unsupported",
        }
    }
}

impl std::fmt::Display for ErrorClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Error)]
pub enum BelltowerError {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("authentication error: {0}")]
    Auth(String),
    #[error("provider error: {0}")]
    Provider(String),
    #[error("tool error: {0}")]
    Tool(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("runtime error: {0}")]
    Runtime(String),
    #[error("invalid state: {0}")]
    InvalidState(String),
    #[error("unsupported operation: {0}")]
    Unsupported(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("toml deserialize error: {0}")]
    TomlDeserialize(#[from] toml::de::Error),
    #[error("toml serialize error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
    #[error("url parse error: {0}")]
    Url(#[from] url::ParseError),
}

impl BelltowerError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Config(_) | Self::TomlDeserialize(_) | Self::TomlSerialize(_) => {
                ErrorClass::Config
            }
            Self::Auth(_) => ErrorClass::Auth,
            Self::Provider(_) => ErrorClass::Provider,
            Self::Tool(_) => ErrorClass::Tool,
            Self::Protocol(_) | Self::Json(_) => ErrorClass::Protocol,
            Self::Storage(_) => ErrorClass::Storage,
            Self::NotFound(_) => ErrorClass::NotFound,
            Self::Runtime(_) | Self::InvalidState(_) | Self::Io(_) | Self::Url(_) => {
                ErrorClass::Runtime
            }
            Self::Unsupported(_) => ErrorClass::Unsupported,
        }
    }

    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Config(_) => "config_error",
            Self::Auth(_) => "auth_error",
            Self::Provider(_) => "provider_error",
            Self::Tool(_) => "tool_error",
            Self::Protocol(_) => "protocol_error",
            Self::Storage(_) => "storage_error",
            Self::NotFound(_) => "not_found",
            Self::Runtime(_) => "runtime_error",
            Self::InvalidState(_) => "invalid_state",
            Self::Unsupported(_) => "unsupported_operation",
            Self::Io(_) => "io_error",
            Self::Json(_) => "json_error",
            Self::TomlDeserialize(_) => "toml_deserialize_error",
            Self::TomlSerialize(_) => "toml_serialize_error",
            Self::Url(_) => "url_parse_error",
        }
    }

    #[must_use]
    pub const fn retryable(&self) -> bool {
        match self {
            Self::Provider(_) | Self::Protocol(_) | Self::Storage(_) | Self::Runtime(_) => true,
            Self::NotFound(_) => false,
            Self::Io(_) | Self::InvalidState(_) => true,
            Self::Config(_)
            | Self::Auth(_)
            | Self::Tool(_)
            | Self::Unsupported(_)
            | Self::Json(_)
            | Self::TomlDeserialize(_)
            | Self::TomlSerialize(_)
            | Self::Url(_) => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, BelltowerError>;
