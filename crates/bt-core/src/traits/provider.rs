use crate::{
    CompletionChunk, CompletionRequest, CompletionSummary, ConnectionStatus, ModelPricing, Result,
};
use futures_core::Stream;
use std::future::Future;
use std::pin::Pin;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type BoxStream<T> = Pin<Box<dyn Stream<Item = T> + Send>>;

pub trait Provider: Send + Sync {
    fn provider_id(&self) -> &str;
    fn validate(&self) -> BoxFuture<'_, Result<ConnectionStatus>>;
    fn list_models(&self) -> BoxFuture<'_, Result<Vec<String>>> {
        Box::pin(async move { Ok(Vec::new()) })
    }
    fn stream_completion(
        &self,
        request: CompletionRequest,
    ) -> BoxFuture<'_, Result<BoxStream<Result<CompletionChunk>>>>;
    fn pricing(&self, model: &str) -> Option<ModelPricing>;
    fn completion_summary(&self, summary: CompletionSummary) -> Result<CompletionSummary> {
        Ok(summary)
    }
}
