use crate::{Credentials, DetectionResult, Result};

pub trait AuthStore: Send + Sync {
    fn get_credentials(
        &self,
        provider: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<Credentials>>> + Send + '_>>;
    fn store_credentials(
        &self,
        provider: &str,
        credentials: Credentials,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>>;
    fn clear_credentials(
        &self,
        provider: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>>;
    fn detect_credentials(
        &self,
        provider: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<DetectionResult>> + Send + '_>>;
}
