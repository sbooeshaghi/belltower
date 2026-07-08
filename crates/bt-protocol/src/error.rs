use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("unsupported protocol version `{requested}`; supported `{supported}`")]
    UnsupportedVersion {
        requested: String,
        supported: &'static str,
    },
    #[error("invalid request: {0}")]
    InvalidRequest(String),
}

pub type Result<T> = std::result::Result<T, ProtocolError>;
