use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub type OpenApiSchemaMap = BTreeMap<String, Value>;

#[must_use]
pub fn openapi_schema_components() -> OpenApiSchemaMap {
    let mut schemas = BTreeMap::new();

    for name in [
        "ApprovalDecision",
        "ApprovalScope",
        "BranchInspection",
        "BranchRecord",
        "BudgetConfig",
        "ConnectionDescriptor",
        "ConnectionModelInventory",
        "EventEnvelope",
        "McpServerDescriptor",
        "McpToolDescriptor",
        "Message",
        "ModelBackendDescriptor",
        "ModelRecommendationsReport",
        "QueuedMessageInspection",
        "RawSseEnvelope",
        "SessionExecutionInspection",
        "SessionInspection",
        "SessionLineageInspection",
        "SessionQueueInspection",
        "SessionRecord",
        "SessionSearchMatch",
        "SessionToolCallInspection",
        "SessionToolMode",
        "SessionTreeInspection",
        "SessionWorkflowInspection",
        "StatusInspection",
        "TurnInspection",
    ] {
        schemas.insert(name.to_owned(), external_schema(name));
    }

    schemas.insert(
        "ExportFormat".to_owned(),
        string_enum_schema(["legacy-bundle", "jsonl", "html", "sharegpt", "otlp"]),
    );
    schemas.insert(
        "ErrorEnvelope".to_owned(),
        object_schema(
            [
                ("class", string_schema()),
                ("code", string_schema()),
                ("message", string_schema()),
                ("retryable", boolean_schema()),
                ("details", nullable(any_schema())),
            ],
            ["class", "code", "message", "retryable"],
        ),
    );
    schemas.insert(
        "CreateSessionRequest".to_owned(),
        object_schema(
            [
                ("project_root", string_schema()),
                ("connection_id", string_schema()),
                ("model_id", nullable(string_schema())),
                ("tool_mode", nullable(ref_schema("SessionToolMode"))),
                ("display_name", nullable(string_schema())),
                ("objective", nullable(string_schema())),
                ("budget", nullable(ref_schema("BudgetConfig"))),
            ],
            ["project_root", "connection_id"],
        ),
    );
    schemas.insert(
        "CreateSessionResponse".to_owned(),
        object_schema(
            [
                ("session", ref_schema("SessionRecord")),
                ("branch", ref_schema("BranchRecord")),
            ],
            ["session", "branch"],
        ),
    );
    schemas.insert(
        "SpawnSessionRequest".to_owned(),
        object_schema(
            [
                ("parent_branch_id", string_schema()),
                ("parent_turn_id", nullable(string_schema())),
                ("objective", string_schema()),
                ("display_name", nullable(string_schema())),
                ("connection_id", nullable(string_schema())),
                ("model_id", nullable(string_schema())),
            ],
            ["parent_branch_id", "objective"],
        ),
    );
    schemas.insert(
        "SpawnSessionResponse".to_owned(),
        object_schema(
            [
                ("parent_session_id", string_schema()),
                ("parent_branch_id", string_schema()),
                ("parent_turn_id", nullable(string_schema())),
                ("child_session", ref_schema("SessionRecord")),
                ("child_branch", ref_schema("BranchRecord")),
            ],
            [
                "parent_session_id",
                "parent_branch_id",
                "child_session",
                "child_branch",
            ],
        ),
    );
    schemas.insert(
        "ListSessionsResponse".to_owned(),
        object_schema(
            [("sessions", array_schema(ref_schema("SessionRecord")))],
            ["sessions"],
        ),
    );
    schemas.insert(
        "SessionInspectionResponse".to_owned(),
        object_schema(
            [("inspection", ref_schema("SessionInspection"))],
            ["inspection"],
        ),
    );
    schemas.insert(
        "SessionTurnsResponse".to_owned(),
        object_schema(
            [
                ("session_id", string_schema()),
                ("turns", array_schema(ref_schema("TurnInspection"))),
            ],
            ["session_id", "turns"],
        ),
    );
    schemas.insert(
        "SessionExecutionResponse".to_owned(),
        single_inspection_response("SessionExecutionInspection"),
    );
    schemas.insert(
        "SessionQueueResponse".to_owned(),
        single_inspection_response("SessionQueueInspection"),
    );
    schemas.insert(
        "SessionQueueClearResponse".to_owned(),
        object_schema(
            [
                ("session_id", string_schema()),
                (
                    "cleared_messages",
                    array_schema(ref_schema("QueuedMessageInspection")),
                ),
            ],
            ["session_id", "cleared_messages"],
        ),
    );
    schemas.insert(
        "SessionToolCallResponse".to_owned(),
        single_inspection_response("SessionToolCallInspection"),
    );
    schemas.insert(
        "SessionSearchResponse".to_owned(),
        object_schema(
            [
                ("session_id", string_schema()),
                ("branch_id", nullable(string_schema())),
                ("query", string_schema()),
                ("limit", integer_schema()),
                ("results", array_schema(ref_schema("SessionSearchMatch"))),
            ],
            ["session_id", "query", "limit", "results"],
        ),
    );
    schemas.insert(
        "SessionLineageResponse".to_owned(),
        single_inspection_response("SessionLineageInspection"),
    );
    schemas.insert(
        "SessionWorkflowResponse".to_owned(),
        single_inspection_response("SessionWorkflowInspection"),
    );
    schemas.insert(
        "SessionBranchInspectionResponse".to_owned(),
        object_schema(
            [
                ("session_id", string_schema()),
                ("active_branch_id", nullable(string_schema())),
                ("branches", array_schema(ref_schema("BranchInspection"))),
            ],
            ["session_id", "branches"],
        ),
    );
    schemas.insert(
        "SessionTreeResponse".to_owned(),
        single_inspection_response("SessionTreeInspection"),
    );
    schemas.insert(
        "UpdateSessionRequest".to_owned(),
        object_schema(
            [
                ("branch_id", string_schema()),
                ("connection_id", nullable(string_schema())),
                ("model_id", nullable(string_schema())),
                ("tool_mode", nullable(ref_schema("SessionToolMode"))),
                ("reset_model_to_default", boolean_schema()),
            ],
            ["branch_id", "reset_model_to_default"],
        ),
    );
    schemas.insert(
        "UpdateSessionBudgetRequest".to_owned(),
        object_schema(
            [
                ("branch_id", string_schema()),
                ("budget", ref_schema("BudgetConfig")),
            ],
            ["branch_id", "budget"],
        ),
    );
    schemas.insert(
        "SendMessageRequest".to_owned(),
        object_schema(
            [
                ("branch_id", string_schema()),
                ("message", ref_schema("Message")),
            ],
            ["branch_id", "message"],
        ),
    );
    schemas.insert(
        "SendMessageOutcome".to_owned(),
        json!({
            "oneOf": [
                {
                    "type": "object",
                    "required": ["outcome"],
                    "properties": { "outcome": { "const": "dispatched" } }
                },
                {
                    "type": "object",
                    "required": ["outcome", "position"],
                    "properties": {
                        "outcome": { "const": "queued" },
                        "position": integer_schema()
                    }
                }
            ]
        }),
    );
    schemas.insert(
        "SendMessageResponse".to_owned(),
        object_schema(
            [
                ("session_id", string_schema()),
                ("branch_id", string_schema()),
                ("outcome", string_schema()),
                ("position", integer_schema()),
            ],
            ["session_id", "branch_id", "outcome"],
        ),
    );
    schemas.insert(
        "CreateBranchRequest".to_owned(),
        object_schema(
            [
                ("from_branch_id", string_schema()),
                ("from_event_id", string_schema()),
                ("activate", boolean_schema()),
                ("carry_summary", boolean_schema()),
            ],
            ["from_branch_id", "activate", "carry_summary"],
        ),
    );
    schemas.insert(
        "CreateBranchResponse".to_owned(),
        object_schema([("branch", ref_schema("BranchRecord"))], ["branch"]),
    );
    schemas.insert(
        "CompactSessionRequest".to_owned(),
        object_schema([("branch_id", string_schema())], ["branch_id"]),
    );
    schemas.insert(
        "ContextCompactionDto".to_owned(),
        object_schema(
            [
                ("compaction_id", string_schema()),
                ("trigger", string_schema()),
                ("phase", string_schema()),
                ("status", string_schema()),
                ("reason", nullable(string_schema())),
                ("provider", nullable(string_schema())),
                ("model", nullable(string_schema())),
                ("context_boundary_seq_id", nullable(integer_schema())),
                ("summary_message_id", nullable(string_schema())),
                ("first_kept_message_id", nullable(string_schema())),
                ("first_kept_branch_id", nullable(string_schema())),
                ("first_kept_seq_id", nullable(integer_schema())),
                ("latency_ms", nullable(integer_schema())),
                ("summary", string_schema()),
                ("messages_before", integer_schema()),
                ("messages_after", integer_schema()),
                ("tokens_before", integer_schema()),
                ("tokens_after", integer_schema()),
                ("files_read", array_schema(string_schema())),
                ("files_modified", array_schema(string_schema())),
            ],
            [
                "compaction_id",
                "trigger",
                "phase",
                "status",
                "reason",
                "provider",
                "model",
                "context_boundary_seq_id",
                "summary_message_id",
                "first_kept_message_id",
                "first_kept_branch_id",
                "first_kept_seq_id",
                "latency_ms",
                "summary",
                "messages_before",
                "messages_after",
                "tokens_before",
                "tokens_after",
                "files_read",
                "files_modified",
            ],
        ),
    );
    schemas.insert(
        "CompactSessionResponse".to_owned(),
        object_schema(
            [
                ("session_id", string_schema()),
                ("branch_id", string_schema()),
                ("model_id", string_schema()),
                ("compaction", nullable(ref_schema("ContextCompactionDto"))),
            ],
            ["session_id", "branch_id", "model_id"],
        ),
    );
    schemas.insert(
        "ActivateBranchRequest".to_owned(),
        object_schema([("carry_summary", boolean_schema())], ["carry_summary"]),
    );
    schemas.insert(
        "ApproveToolRequest".to_owned(),
        object_schema(
            [
                ("call_id", string_schema()),
                ("tool_name", string_schema()),
                ("scope", ref_schema("ApprovalScope")),
                ("decision", ref_schema("ApprovalDecision")),
            ],
            ["call_id", "tool_name", "scope", "decision"],
        ),
    );
    schemas.insert(
        "AnswerToolRequest".to_owned(),
        object_schema(
            [("call_id", string_schema()), ("response", any_schema())],
            ["call_id", "response"],
        ),
    );
    schemas.insert(
        "CancelSessionRequest".to_owned(),
        object_schema(
            [
                ("branch_id", string_schema()),
                ("reason", nullable(string_schema())),
            ],
            ["branch_id"],
        ),
    );
    schemas.insert(
        "SteerSessionRequest".to_owned(),
        object_schema(
            [("branch_id", string_schema()), ("message", string_schema())],
            ["branch_id", "message"],
        ),
    );
    schemas.insert(
        "RecordOperatorCommandRequest".to_owned(),
        object_schema(
            [
                ("branch_id", string_schema()),
                ("command_type", string_schema()),
                ("raw_input", string_schema()),
                ("output", string_schema()),
                ("success", boolean_schema()),
            ],
            [
                "branch_id",
                "command_type",
                "raw_input",
                "output",
                "success",
            ],
        ),
    );
    schemas.insert(
        "RunShellCommandRequest".to_owned(),
        object_schema(
            [
                ("branch_id", string_schema()),
                ("raw_input", string_schema()),
                ("command", string_schema()),
                ("timeout_seconds", nullable(integer_schema())),
            ],
            ["branch_id", "raw_input", "command"],
        ),
    );
    schemas.insert(
        "SessionEventsResponse".to_owned(),
        object_schema(
            [
                ("session_id", string_schema()),
                ("events", array_schema(ref_schema("EventEnvelope"))),
                ("last_seq_id", nullable(integer_schema())),
            ],
            ["session_id", "events"],
        ),
    );
    schemas.insert(
        "BranchOperatorCommandsResponse".to_owned(),
        object_schema(
            [
                ("session_id", string_schema()),
                ("branch_id", string_schema()),
                ("events", array_schema(ref_schema("EventEnvelope"))),
                ("last_seq_id", nullable(integer_schema())),
            ],
            ["session_id", "branch_id", "events"],
        ),
    );
    schemas.insert(
        "SequencedMessageDto".to_owned(),
        object_schema(
            [
                ("seq_id", integer_schema()),
                ("message", ref_schema("Message")),
            ],
            ["seq_id", "message"],
        ),
    );
    schemas.insert(
        "RecordedOperatorCommandDto".to_owned(),
        object_schema(
            [
                ("seq_id", integer_schema()),
                ("occurred_at", date_time_schema()),
                ("command_type", string_schema()),
                ("raw_input", string_schema()),
                ("output", string_schema()),
                ("success", boolean_schema()),
            ],
            [
                "seq_id",
                "occurred_at",
                "command_type",
                "raw_input",
                "output",
                "success",
            ],
        ),
    );
    schemas.insert(
        "BranchMessagesPageResponse".to_owned(),
        object_schema(
            [
                ("session_id", string_schema()),
                ("branch_id", string_schema()),
                ("messages", array_schema(ref_schema("SequencedMessageDto"))),
                ("oldest_seq_id", nullable(integer_schema())),
                ("newest_seq_id", nullable(integer_schema())),
                ("has_more_before", boolean_schema()),
                ("last_seq_id", nullable(integer_schema())),
            ],
            ["session_id", "branch_id", "messages", "has_more_before"],
        ),
    );
    schemas.insert(
        "BranchOperatorCommandsPageResponse".to_owned(),
        object_schema(
            [
                ("session_id", string_schema()),
                ("branch_id", string_schema()),
                (
                    "commands",
                    array_schema(ref_schema("RecordedOperatorCommandDto")),
                ),
                ("oldest_seq_id", nullable(integer_schema())),
                ("newest_seq_id", nullable(integer_schema())),
                ("has_more_before", boolean_schema()),
                ("last_seq_id", nullable(integer_schema())),
            ],
            ["session_id", "branch_id", "commands", "has_more_before"],
        ),
    );
    schemas.insert(
        "SessionMessagesResponse".to_owned(),
        object_schema(
            [
                ("session_id", string_schema()),
                ("branch_id", nullable(string_schema())),
                ("messages", array_schema(ref_schema("Message"))),
            ],
            ["session_id", "messages"],
        ),
    );
    schemas.insert(
        "RawChunkDto".to_owned(),
        object_schema(
            [
                ("chunk_id", integer_schema()),
                ("branch_id", nullable(string_schema())),
                ("turn_id", nullable(string_schema())),
                ("llm_call_ordinal", nullable(integer_schema())),
                ("event_id", nullable(string_schema())),
                ("provider", string_schema()),
                ("stream_name", string_schema()),
                ("content_base64", string_schema()),
                ("received_at", date_time_schema()),
            ],
            [
                "chunk_id",
                "provider",
                "stream_name",
                "content_base64",
                "received_at",
            ],
        ),
    );
    schemas.insert(
        "SessionRawChunksResponse".to_owned(),
        object_schema(
            [
                ("session_id", string_schema()),
                ("chunks", array_schema(ref_schema("RawChunkDto"))),
            ],
            ["session_id", "chunks"],
        ),
    );
    schemas.insert(
        "TurnRawChunksPageResponse".to_owned(),
        object_schema(
            [
                ("session_id", string_schema()),
                ("branch_id", string_schema()),
                ("turn_id", string_schema()),
                ("chunks", array_schema(ref_schema("RawChunkDto"))),
                ("oldest_chunk_id", nullable(integer_schema())),
                ("newest_chunk_id", nullable(integer_schema())),
                ("has_more_before", boolean_schema()),
            ],
            [
                "session_id",
                "branch_id",
                "turn_id",
                "chunks",
                "has_more_before",
            ],
        ),
    );
    schemas.insert(
        "SessionExportResponse".to_owned(),
        object_schema(
            [
                ("session_id", string_schema()),
                ("format", ref_schema("ExportFormat")),
                ("content_type", string_schema()),
                ("content", string_schema()),
            ],
            ["session_id", "format", "content_type", "content"],
        ),
    );
    schemas.insert(
        "PushOtlpExportRequest".to_owned(),
        object_schema(
            [
                ("endpoint", nullable(string_schema())),
                ("project_name", nullable(string_schema())),
                ("api_key", nullable(string_schema())),
                ("headers", map_schema(string_schema())),
            ],
            [],
        ),
    );
    schemas.insert(
        "PushOtlpExportResponse".to_owned(),
        object_schema(
            [
                ("session_id", string_schema()),
                ("request_url", string_schema()),
                ("content_type", string_schema()),
                ("bytes_sent", integer_schema()),
                ("status_code", integer_schema()),
                ("rejected_spans", nullable(integer_schema())),
                ("warning", nullable(string_schema())),
            ],
            [
                "session_id",
                "request_url",
                "content_type",
                "bytes_sent",
                "status_code",
            ],
        ),
    );
    schemas.insert(
        "BranchesResponse".to_owned(),
        object_schema(
            [
                ("session_id", string_schema()),
                ("branches", array_schema(ref_schema("BranchRecord"))),
            ],
            ["session_id", "branches"],
        ),
    );
    schemas.insert(
        "ConnectionsResponse".to_owned(),
        object_schema(
            [(
                "connections",
                array_schema(ref_schema("ConnectionDescriptor")),
            )],
            ["connections"],
        ),
    );
    schemas.insert(
        "StatusInspectionResponse".to_owned(),
        object_schema(
            [("inspection", ref_schema("StatusInspection"))],
            ["inspection"],
        ),
    );
    schemas.insert(
        "McpServersResponse".to_owned(),
        object_schema(
            [("servers", array_schema(ref_schema("McpServerDescriptor")))],
            ["servers"],
        ),
    );
    schemas.insert(
        "McpToolsResponse".to_owned(),
        object_schema(
            [("tools", array_schema(ref_schema("McpToolDescriptor")))],
            ["tools"],
        ),
    );
    schemas.insert(
        "McpInventoryResponse".to_owned(),
        object_schema(
            [
                ("servers", array_schema(ref_schema("McpServerDescriptor"))),
                ("tools", array_schema(ref_schema("McpToolDescriptor"))),
            ],
            ["servers", "tools"],
        ),
    );
    schemas.insert(
        "ModelBackendsResponse".to_owned(),
        object_schema(
            [(
                "backends",
                array_schema(ref_schema("ModelBackendDescriptor")),
            )],
            ["backends"],
        ),
    );
    schemas.insert(
        "ModelRecommendationsResponse".to_owned(),
        object_schema(
            [("report", ref_schema("ModelRecommendationsReport"))],
            ["report"],
        ),
    );
    schemas.insert(
        "ConnectionModelsResponse".to_owned(),
        object_schema(
            [(
                "connections",
                array_schema(ref_schema("ConnectionModelInventory")),
            )],
            ["connections"],
        ),
    );
    schemas.insert(
        "HealthResponse".to_owned(),
        object_schema(
            [
                ("status", string_schema()),
                ("protocol_version", string_schema()),
            ],
            ["status", "protocol_version"],
        ),
    );
    schemas.insert(
        "ServerCapabilities".to_owned(),
        object_schema(
            [
                ("approvals", boolean_schema()),
                ("pending_input", boolean_schema()),
                ("session_queue", boolean_schema()),
                ("workflow_inspection", boolean_schema()),
                ("lineage_inspection", boolean_schema()),
                ("raw_chunk_paging", boolean_schema()),
                ("exports", boolean_schema()),
                ("mcp_inventory", boolean_schema()),
                ("mcp_reload", boolean_schema()),
                ("spawn_session", boolean_schema()),
            ],
            [
                "approvals",
                "pending_input",
                "session_queue",
                "workflow_inspection",
                "lineage_inspection",
                "raw_chunk_paging",
                "exports",
                "mcp_inventory",
                "mcp_reload",
                "spawn_session",
            ],
        ),
    );
    schemas.insert(
        "ServerInfoResponse".to_owned(),
        object_schema(
            [
                ("server_version", string_schema()),
                ("protocol_version", string_schema()),
                ("supported_protocol_versions", array_schema(string_schema())),
                ("capabilities", ref_schema("ServerCapabilities")),
            ],
            [
                "server_version",
                "protocol_version",
                "supported_protocol_versions",
                "capabilities",
            ],
        ),
    );
    schemas
}

fn single_inspection_response(name: &str) -> Value {
    object_schema([("inspection", ref_schema(name))], ["inspection"])
}

fn object_schema<const N: usize, const R: usize>(
    properties: [(&str, Value); N],
    required: [&str; R],
) -> Value {
    let mut props = Map::new();
    for (name, schema) in properties {
        props.insert(name.to_owned(), schema);
    }

    let required = required.into_iter().collect::<Vec<_>>();
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": props,
        "required": required,
    })
}

fn external_schema(name: &str) -> Value {
    json!({
        "type": "object",
        "description": format!("{name} is defined in bt-core or bt-protocol and serialized through bt-protocol.")
    })
}

fn string_enum_schema<const N: usize>(values: [&str; N]) -> Value {
    json!({
        "type": "string",
        "enum": values.into_iter().collect::<Vec<_>>(),
    })
}

fn ref_schema(name: &str) -> Value {
    json!({ "$ref": format!("#/components/schemas/{name}") })
}

fn array_schema(items: Value) -> Value {
    json!({ "type": "array", "items": items })
}

fn map_schema(values: Value) -> Value {
    json!({ "type": "object", "additionalProperties": values })
}

fn nullable(schema: Value) -> Value {
    json!({ "anyOf": [schema, { "type": "null" }] })
}

fn any_schema() -> Value {
    json!({})
}

fn string_schema() -> Value {
    json!({ "type": "string" })
}

fn date_time_schema() -> Value {
    json!({ "type": "string", "format": "date-time" })
}

fn integer_schema() -> Value {
    json!({ "type": "integer" })
}

fn boolean_schema() -> Value {
    json!({ "type": "boolean" })
}
