use bt_protocol::{PROTOCOL_VERSION, openapi_schema_components};
use serde_json::{Map, Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
}

impl HttpMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteSpec {
    pub method: HttpMethod,
    pub path: &'static str,
    pub tag: &'static str,
    pub summary: &'static str,
    pub request: Option<&'static str>,
    pub response: Option<&'static str>,
    pub auth_required: bool,
}

#[must_use]
pub const fn route_specs() -> &'static [RouteSpec] {
    &ROUTES
}

#[must_use]
pub fn openapi_document() -> Value {
    let mut paths = Map::new();
    for route in route_specs() {
        let entry = paths
            .entry(route.path.to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        let path_item = entry.as_object_mut().expect("path item must be an object");
        path_item.insert(route.method.as_str().to_owned(), operation(route));
    }

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Belltower Server API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Public HTTP and SSE API for Belltower clients."
        },
        "servers": [
            {
                "url": "http://127.0.0.1:7400",
                "description": "Default local Belltower server"
            }
        ],
        "paths": paths,
        "components": {
            "securitySchemes": {
                "BearerAuth": {
                    "type": "http",
                    "scheme": "bearer"
                }
            },
            "schemas": openapi_schema_components()
        },
        "x-belltower-protocol-version": PROTOCOL_VERSION,
    })
}

#[must_use]
#[allow(dead_code)]
pub fn openapi_json_pretty() -> String {
    let mut text =
        serde_json::to_string_pretty(&openapi_document()).expect("OpenAPI document serializes");
    text.push('\n');
    text
}

fn operation(route: &RouteSpec) -> Value {
    let mut operation = Map::new();
    operation.insert("summary".to_owned(), json!(route.summary));
    operation.insert("tags".to_owned(), json!([route.tag]));
    operation.insert("parameters".to_owned(), json!(path_parameters(route.path)));
    if route.auth_required {
        operation.insert("security".to_owned(), json!([{ "BearerAuth": [] }]));
    }
    if let Some(request) = route.request {
        operation.insert(
            "requestBody".to_owned(),
            json!({
                "required": true,
                "content": {
                    "application/json": {
                        "schema": schema_ref(request)
                    }
                }
            }),
        );
    }
    operation.insert("responses".to_owned(), responses(route));
    Value::Object(operation)
}

fn responses(route: &RouteSpec) -> Value {
    let mut responses = Map::new();
    let success = if route.path == "/sessions/{session_id}/events/stream" {
        json!({
            "description": "Successful response",
            "content": {
                "text/event-stream": {
                    "schema": schema_ref(route.response.expect("SSE route has an envelope schema"))
                }
            }
        })
    } else {
        match route.response {
            Some(schema) => json!({
                "description": "Successful response",
                "content": {
                    "application/json": {
                        "schema": schema_ref(schema)
                    }
                }
            }),
            None => json!({ "description": "Accepted with no response body" }),
        }
    };
    let success_code = if route.response.is_none() || route.path == "/sessions/{session_id}/message"
    {
        "202"
    } else {
        "200"
    };
    responses.insert(success_code.to_owned(), success);
    responses.insert(
        "default".to_owned(),
        json!({
            "description": "Belltower error response",
            "content": {
                "application/json": {
                    "schema": schema_ref("ErrorEnvelope")
                }
            }
        }),
    );
    Value::Object(responses)
}

fn schema_ref(name: &str) -> Value {
    json!({ "$ref": format!("#/components/schemas/{name}") })
}

fn path_parameters(path: &str) -> Vec<Value> {
    path.split('/')
        .filter_map(|segment| {
            let name = segment.strip_prefix('{')?.strip_suffix('}')?;
            Some(json!({
                "name": name,
                "in": "path",
                "required": true,
                "schema": { "type": "string" }
            }))
        })
        .collect()
}

const ROUTES: [RouteSpec; 50] = [
    RouteSpec {
        method: HttpMethod::Get,
        path: "/health",
        tag: "server",
        summary: "Health check and protocol version.",
        request: None,
        response: Some("HealthResponse"),
        auth_required: false,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/server/info",
        tag: "server",
        summary: "Server version and capability metadata.",
        request: None,
        response: Some("ServerInfoResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions",
        tag: "sessions",
        summary: "List sessions.",
        request: None,
        response: Some("ListSessionsResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Post,
        path: "/sessions",
        tag: "sessions",
        summary: "Create a session.",
        request: Some("CreateSessionRequest"),
        response: Some("CreateSessionResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}",
        tag: "sessions",
        summary: "Inspect a session.",
        request: None,
        response: Some("SessionInspectionResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Post,
        path: "/sessions/{session_id}",
        tag: "sessions",
        summary: "Update session settings.",
        request: Some("UpdateSessionRequest"),
        response: Some("CreateSessionResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Post,
        path: "/sessions/{session_id}/budget",
        tag: "control",
        summary: "Configure session budget limits.",
        request: Some("UpdateSessionBudgetRequest"),
        response: Some("SessionInspectionResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Post,
        path: "/sessions/{session_id}/message",
        tag: "sessions",
        summary: "Send or queue a user message.",
        request: Some("SendMessageRequest"),
        response: Some("SendMessageResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Post,
        path: "/sessions/{session_id}/spawn",
        tag: "sessions",
        summary: "Spawn a child session.",
        request: Some("SpawnSessionRequest"),
        response: Some("SpawnSessionResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Post,
        path: "/sessions/{session_id}/approve",
        tag: "control",
        summary: "Resolve a pending tool approval.",
        request: Some("ApproveToolRequest"),
        response: None,
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Post,
        path: "/sessions/{session_id}/answer",
        tag: "control",
        summary: "Answer a pending operator question.",
        request: Some("AnswerToolRequest"),
        response: None,
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/branches",
        tag: "branches",
        summary: "List session branches.",
        request: None,
        response: Some("BranchesResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Post,
        path: "/sessions/{session_id}/branches",
        tag: "branches",
        summary: "Create a session branch.",
        request: Some("CreateBranchRequest"),
        response: Some("CreateBranchResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Post,
        path: "/sessions/{session_id}/compact",
        tag: "branches",
        summary: "Compact branch context.",
        request: Some("CompactSessionRequest"),
        response: Some("CompactSessionResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/branches/inspect",
        tag: "branches",
        summary: "Inspect session branches.",
        request: None,
        response: Some("SessionBranchInspectionResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Post,
        path: "/sessions/{session_id}/branches/{branch_id}/activate",
        tag: "branches",
        summary: "Activate a branch.",
        request: Some("ActivateBranchRequest"),
        response: None,
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/branches/{branch_id}/messages",
        tag: "transcript",
        summary: "Read branch messages.",
        request: None,
        response: Some("SessionMessagesResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/branches/{branch_id}/messages/page",
        tag: "transcript",
        summary: "Page branch messages.",
        request: None,
        response: Some("BranchMessagesPageResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/branches/{branch_id}/commands",
        tag: "operator",
        summary: "Read branch operator commands.",
        request: None,
        response: Some("BranchOperatorCommandsResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/branches/{branch_id}/commands/page",
        tag: "operator",
        summary: "Page branch operator commands.",
        request: None,
        response: Some("BranchOperatorCommandsPageResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/raw_chunks",
        tag: "telemetry",
        summary: "List raw provider chunks for a session.",
        request: None,
        response: Some("SessionRawChunksResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/branches/{branch_id}/turns/{turn_id}/raw_chunks/page",
        tag: "telemetry",
        summary: "Page raw provider chunks for a turn.",
        request: None,
        response: Some("TurnRawChunksPageResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/export/{format}",
        tag: "exports",
        summary: "Export a session.",
        request: None,
        response: Some("SessionExportResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Post,
        path: "/sessions/{session_id}/export/otlp/push",
        tag: "exports",
        summary: "Push an OTLP export to a collector.",
        request: Some("PushOtlpExportRequest"),
        response: Some("PushOtlpExportResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/events",
        tag: "events",
        summary: "Replay stored session events.",
        request: None,
        response: Some("SessionEventsResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/events/stream",
        tag: "events",
        summary: "Subscribe to session events as SSE.",
        request: None,
        response: Some("RawSseEnvelope"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/turns",
        tag: "inspection",
        summary: "Inspect session turns.",
        request: None,
        response: Some("SessionTurnsResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/execution",
        tag: "inspection",
        summary: "Inspect live and replayable execution state.",
        request: None,
        response: Some("SessionExecutionResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/queue",
        tag: "control",
        summary: "Inspect pending control-plane state.",
        request: None,
        response: Some("SessionQueueResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Post,
        path: "/sessions/{session_id}/queue/clear",
        tag: "control",
        summary: "Clear queued user messages.",
        request: None,
        response: Some("SessionQueueClearResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/tool-calls/{call_id}",
        tag: "tools",
        summary: "Inspect a tool call.",
        request: None,
        response: Some("SessionToolCallResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/search",
        tag: "inspection",
        summary: "Search canonical session history.",
        request: None,
        response: Some("SessionSearchResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/lineage",
        tag: "inspection",
        summary: "Inspect session lineage.",
        request: None,
        response: Some("SessionLineageResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/workflow",
        tag: "inspection",
        summary: "Inspect workflow state.",
        request: None,
        response: Some("SessionWorkflowResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/tree",
        tag: "inspection",
        summary: "Inspect session tree.",
        request: None,
        response: Some("SessionTreeResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/sessions/{session_id}/messages",
        tag: "transcript",
        summary: "Read active branch messages.",
        request: None,
        response: Some("SessionMessagesResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Post,
        path: "/sessions/{session_id}/cancel",
        tag: "control",
        summary: "Request session cancellation.",
        request: Some("CancelSessionRequest"),
        response: None,
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Post,
        path: "/sessions/{session_id}/steer",
        tag: "control",
        summary: "Queue steering input for a session.",
        request: Some("SteerSessionRequest"),
        response: None,
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Post,
        path: "/sessions/{session_id}/commands",
        tag: "operator",
        summary: "Record a durable operator command.",
        request: Some("RecordOperatorCommandRequest"),
        response: None,
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Post,
        path: "/sessions/{session_id}/commands/shell",
        tag: "operator",
        summary: "Run and record an operator shell command.",
        request: Some("RunShellCommandRequest"),
        response: None,
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/connections",
        tag: "providers",
        summary: "List configured connections.",
        request: None,
        response: Some("ConnectionsResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/status/inspect",
        tag: "providers",
        summary: "Inspect readiness status.",
        request: None,
        response: Some("StatusInspectionResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/mcp",
        tag: "mcp",
        summary: "Inspect MCP inventory.",
        request: None,
        response: Some("McpInventoryResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/mcp/servers",
        tag: "mcp",
        summary: "List MCP servers.",
        request: None,
        response: Some("McpServersResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/mcp/tools",
        tag: "mcp",
        summary: "List MCP tools.",
        request: None,
        response: Some("McpToolsResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Post,
        path: "/mcp/reload",
        tag: "mcp",
        summary: "Reload MCP configuration.",
        request: None,
        response: None,
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/models/backends",
        tag: "models",
        summary: "List model backends.",
        request: None,
        response: Some("ModelBackendsResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/models/connections",
        tag: "models",
        summary: "List models by connection.",
        request: None,
        response: Some("ConnectionModelsResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/models/connections/{connection_id}",
        tag: "models",
        summary: "Inspect model availability for one connection.",
        request: None,
        response: Some("ConnectionModelsResponse"),
        auth_required: true,
    },
    RouteSpec {
        method: HttpMethod::Get,
        path: "/models/recommendations",
        tag: "models",
        summary: "List model recommendations.",
        request: None,
        response: Some("ModelRecommendationsResponse"),
        auth_required: true,
    },
];
