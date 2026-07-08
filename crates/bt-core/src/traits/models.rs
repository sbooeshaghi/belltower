use crate::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BackendStatus {
    pub backend: String,
    pub running: bool,
    pub base_url: String,
    pub models_loaded: Vec<String>,
    pub models_available: Vec<String>,
}

pub trait ModelManager: Send + Sync {
    fn detect_backends(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<BackendStatus>>> + Send + '_>>;
}
