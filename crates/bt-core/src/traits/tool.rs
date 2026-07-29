use crate::{
    ApprovalDecision, ApprovalRequest, ApprovalRequirement, Result, ToolResultEnvelope, ToolSpec,
};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<ToolResultEnvelope>> + Send + 'a>>;

/// A cheap, runtime-owned signal for interrupting one exact active turn.
///
/// The durable event log remains the authority for whether cancellation was
/// requested. This process-local signal only wakes work that is currently in
/// flight so it can converge on that canonical state promptly.
#[derive(Clone, Debug, Default)]
pub struct CancellationSignal {
    cancelled: Arc<AtomicBool>,
}

impl CancellationSignal {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Clone, Debug)]
pub struct ToolContext {
    pub project_root: camino::Utf8PathBuf,
    pub cancellation: Option<CancellationSignal>,
}

impl ToolContext {
    #[must_use]
    pub fn new(project_root: camino::Utf8PathBuf) -> Self {
        Self {
            project_root,
            cancellation: None,
        }
    }

    #[must_use]
    pub fn with_cancellation(mut self, cancellation: CancellationSignal) -> Self {
        self.cancellation = Some(cancellation);
        self
    }
}

pub trait ToolExecutor: Send + Sync {
    fn spec(&self) -> ToolSpec;
    fn approval_requirement(&self, arguments: &Value) -> ApprovalRequirement;
    fn execute(&self, arguments: Value, context: ToolContext) -> ToolFuture<'_>;
}

pub trait ApprovalEvaluator: Send + Sync {
    fn evaluate(&self, request: &ApprovalRequest) -> Result<Option<ApprovalDecision>>;
}
