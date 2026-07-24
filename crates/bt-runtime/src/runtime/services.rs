// services.rs keeps one bounded runtime concern out of `runtime.rs`; cross-crate ownership remains unchanged.
use super::*;

impl BelltowerRuntime {
    pub fn connection(&self, id: &ConnectionId) -> Option<ConnectionDescriptor> {
        self.connections.get(id)
    }

    pub async fn mcp_servers(&self) -> Vec<McpServerDescriptor> {
        self.mcp.servers().await
    }

    pub async fn mcp_tools(&self) -> Result<Vec<McpToolDescriptor>> {
        self.mcp.tools().await
    }

    pub async fn mcp_inventory(&self) -> Result<bt_mcp::McpInventorySnapshot> {
        self.mcp.inventory().await
    }

    pub async fn mcp_registered_tools(&self) -> Result<Vec<McpRegisteredTool>> {
        self.mcp.registered_tools().await
    }

    pub async fn reload_mcp(&self) -> Result<()> {
        self.mcp.reload().await
    }

    pub async fn model_backends(&self) -> Vec<ModelBackendDescriptor> {
        self.models.backends().await
    }

    pub fn model_recommendations(&self) -> Result<ModelRecommendationsReport> {
        self.models.recommendations()
    }

    pub fn connections(&self) -> Vec<bt_core::ConnectionDescriptor> {
        self.connections.all()
    }

    pub fn approval_evaluator(&self) -> Arc<dyn ApprovalEvaluator> {
        self.approval_evaluator.clone()
    }

    /// Puts a session into (or out of) auto-approval mode: every approval
    /// request resolves as a recorded policy decision. Process-local and
    /// fail-safe — restarts fall back to prompting.
    pub fn set_session_approval_auto(&self, session_id: SessionId, enabled: bool) {
        self.approval_evaluator
            .set_session_auto_approval(session_id, enabled);
    }

    #[must_use]
    pub fn session_approval_is_auto(&self, session_id: SessionId) -> bool {
        self.approval_evaluator.session_auto_approval(session_id)
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.event_bus.subscribe()
    }

    #[must_use]
    pub fn config(&self) -> &BelltowerConfig {
        &self.config
    }
}
