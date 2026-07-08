use axum::{Json, Router, routing::post};
use bt_core::{BelltowerConfig, McpServerConfig, McpServerStatus, McpTransportConfig, ToolContext};
use bt_mcp::McpRegistry;
use camino::Utf8PathBuf;
use serde_json::json;
use std::collections::BTreeMap;
use tokio::net::TcpListener;

#[tokio::test]
async fn stdio_registry_discovers_and_executes_tools() {
    let mut config = BelltowerConfig::from_embedded().expect("embedded config");
    config.mcp.servers = vec![McpServerConfig {
        name: "fixture".to_owned(),
        enabled: true,
        transport: McpTransportConfig::Stdio {
            command: env!("CARGO_BIN_EXE_fake_mcp_server").to_owned(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
        },
    }];

    let registry = McpRegistry::from_config(&config);
    let before = registry.servers().await;
    assert!(matches!(before[0].status, McpServerStatus::Configured));

    let tools = registry.registered_tools().await.expect("registered tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].descriptor.server_name, "fixture");
    assert_eq!(tools[0].descriptor.tool_name, "echo_remote");
    assert_eq!(
        tools[0].descriptor.qualified_name,
        "mcp_fixture_echo_remote"
    );

    let result = tools[0]
        .executor
        .execute(
            json!({
                "call_id": "call-1",
                "text": "hello from belltower"
            }),
            ToolContext {
                project_root: Utf8PathBuf::from("/tmp"),
            },
        )
        .await
        .expect("tool execution");
    assert!(!result.is_error);
    assert_eq!(result.output["echoed"], "hello from belltower");

    let after = registry.servers().await;
    assert!(matches!(after[0].status, McpServerStatus::Ready));

    registry.reload().await.expect("reload");
    let reloaded = registry
        .registered_tools()
        .await
        .expect("registered tools after reload");
    assert_eq!(
        reloaded[0].descriptor.qualified_name,
        "mcp_fixture_echo_remote"
    );
}

#[tokio::test]
async fn failing_servers_are_degraded_without_breaking_registry_listing() {
    let mut config = BelltowerConfig::from_embedded().expect("embedded config");
    config.mcp.servers = vec![McpServerConfig {
        name: "broken".to_owned(),
        enabled: true,
        transport: McpTransportConfig::Stdio {
            command: "/bin/false".to_owned(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
        },
    }];

    let registry = McpRegistry::from_config(&config);
    let tools = registry.registered_tools().await.expect("registered tools");
    assert!(tools.is_empty());

    let servers = registry.servers().await;
    assert_eq!(servers.len(), 1);
    assert!(matches!(
        servers[0].status,
        McpServerStatus::Degraded { .. }
    ));
}

#[tokio::test]
async fn http_registry_discovers_and_executes_tools() {
    async fn rpc(Json(request): Json<serde_json::Value>) -> Json<serde_json::Value> {
        let id = request.get("id").cloned();
        let method = request
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let result = match method {
            "initialize" => json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "fake-http-mcp", "version": "0.1.0" }
            }),
            "tools/list" => json!({
                "tools": [{
                    "name": "echo_remote",
                    "description": "Echoes text from HTTP MCP",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "text": { "type": "string" }
                        },
                        "required": ["text"],
                        "additionalProperties": false
                    }
                }]
            }),
            "tools/call" => json!({
                "structuredContent": {
                    "echoed": request["params"]["arguments"]["text"].clone()
                },
                "isError": false
            }),
            _ => json!({}),
        };
        if let Some(id) = id {
            Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result,
            }))
        } else {
            Json(json!({
                "jsonrpc": "2.0",
                "result": {}
            }))
        }
    }

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind http mcp");
    let addr = listener.local_addr().expect("http mcp addr");
    tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/", post(rpc)))
            .await
            .expect("http mcp server");
    });

    let mut config = BelltowerConfig::from_embedded().expect("embedded config");
    config.mcp.servers = vec![McpServerConfig {
        name: "http_fixture".to_owned(),
        enabled: true,
        transport: McpTransportConfig::StreamableHttp {
            base_url: format!("http://{addr}/").parse().expect("http mcp url"),
            headers: BTreeMap::new(),
        },
    }];

    let registry = McpRegistry::from_config(&config);
    let tools = registry.registered_tools().await.expect("registered tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(
        tools[0].descriptor.qualified_name,
        "mcp_http_fixture_echo_remote"
    );

    let result = tools[0]
        .executor
        .execute(
            json!({
                "call_id": "call-http",
                "text": "hello over http"
            }),
            ToolContext {
                project_root: Utf8PathBuf::from("/tmp"),
            },
        )
        .await
        .expect("http tool execution");
    assert!(!result.is_error);
    assert_eq!(result.output["echoed"], "hello over http");

    let servers = registry.servers().await;
    assert!(matches!(servers[0].status, McpServerStatus::Ready));
}
