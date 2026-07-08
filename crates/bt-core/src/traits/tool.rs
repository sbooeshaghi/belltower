use crate::{
    ApprovalDecision, ApprovalRequest, ApprovalRequirement, Result, ToolResultEnvelope, ToolSpec,
};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<ToolResultEnvelope>> + Send + 'a>>;

#[derive(Clone, Debug)]
pub struct ToolContext {
    pub project_root: camino::Utf8PathBuf,
}

pub trait ToolExecutor: Send + Sync {
    fn spec(&self) -> ToolSpec;
    fn approval_requirement(&self, arguments: &Value) -> ApprovalRequirement;
    fn execute(&self, arguments: Value, context: ToolContext) -> ToolFuture<'_>;
}

pub trait ApprovalEvaluator: Send + Sync {
    fn evaluate(&self, request: &ApprovalRequest) -> Result<Option<ApprovalDecision>>;
}
