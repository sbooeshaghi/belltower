//! Server metadata and capability routes.
//!
//! These routes are protocol-facing metadata only; runtime/session semantics
//! stay in the focused route modules that own those surfaces.

use axum::Json;
use bt_protocol::{HealthResponse, PROTOCOL_VERSION, ServerCapabilities, ServerInfoResponse};

pub(super) async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_owned(),
        protocol_version: PROTOCOL_VERSION.to_owned(),
    })
}

fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        approvals: true,
        pending_input: true,
        session_queue: true,
        workflow_inspection: true,
        lineage_inspection: true,
        raw_chunk_paging: true,
        exports: true,
        mcp_inventory: true,
        mcp_reload: true,
        spawn_session: true,
    }
}

pub(super) async fn server_info() -> Json<ServerInfoResponse> {
    Json(ServerInfoResponse {
        server_version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_version: PROTOCOL_VERSION.to_owned(),
        supported_protocol_versions: vec![PROTOCOL_VERSION.to_owned()],
        capabilities: server_capabilities(),
    })
}
