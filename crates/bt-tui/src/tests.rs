use super::{
    COMPOSER_BACKGROUND, ChatAction, ChatApp, ConnectionId, ESCAPE_PREFIX_TIMEOUT, EventEnvelope,
    EventPayload, LoadedTranscriptPage, Message, MessagePart, PendingSendCompletion, Role,
    ScrollbackSeparator, SessionId, SessionToolMode, ToolCallId, ToolResultEnvelope,
    TranscriptDensity, TranscriptEntryKind, command_help_output, completion_for_input,
    composer_body_height, composer_text_area_rect, composer_wrap_width,
    desired_inline_viewport_height, render_bottom_panel, render_doctor_output,
    render_execution_output, render_footer_lines, render_message_with_options,
    render_models_output, render_raw_diff_output, render_session_output, render_status_output,
    render_tool_block_entry, render_tool_call_inspection_output, resolve_command,
    session_readiness_summary, shell_aux_lines, shell_top_gap_height,
};
use bt_client::BelltowerClient;
use bt_core::{
    BranchId, CompletionDelta, ConnectionAuthState, ConnectionDescriptor, ConnectionModelInventory,
    ConnectionModelOption, ConnectionModelSource, ConnectionReadinessInspection,
    ConnectionReadinessState, ConnectionSupportState, ContextManifest, ContextMessageRef,
    ContextSystemPromptRef, ContextToolRef, CredentialKind, ErrorClass, HardwareProfile,
    McpServerDescriptor, McpServerStatus, McpToolDescriptor, MessageId, ModelBackendDescriptor,
    ModelBackendKind, ModelBackendStatus, ModelRecommendation, PendingApprovalInspection,
    PendingInputInspection, SessionExecutionInspection, SessionQueueInspection, SessionRecord,
    SessionRuntimeState, SessionStatus, SessionToolCallInspection, SpanId, SpanKind,
    StatusInspection, TokenUsage, ToolCall, ToolExecutionMode, ToolRiskClass, TraceEventCounts,
    TraceTurnInspection, TurnStartSource, default_settings_revision_id,
};
use bt_protocol::{
    HealthResponse, SendMessageOutcome, SendMessageResponse, SessionExecutionResponse,
    SessionInspectionResponse,
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use ratatui::style::Modifier;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};
use time::OffsetDateTime;

use crate::bottom_pane::BottomSurfaceKind;
use crate::command_actions::parse_spawn_command_args;
use crate::command_render::render_turn_start_source;
use crate::inline_terminal::render_input_text;

fn test_client() -> BelltowerClient {
    BelltowerClient::try_from("http://127.0.0.1:7400/").expect("client")
}

fn test_app() -> ChatApp {
    let mut app = ChatApp::new(
        test_client(),
        SessionId::new(),
        BranchId::new(),
        "/tmp/belltower".to_owned(),
        ConnectionId::new("local"),
    );
    app.set_transcript_output_width(80);
    app.set_input_view_width(composer_wrap_width(80));
    app.set_bottom_panel_view_width(80);
    app.tool_mode = SessionToolMode::Extended;
    app
}

#[test]
fn apply_session_metadata_updates_selection_without_readiness_inventory() {
    let mut app = test_app();
    let session_id = app.session_id;
    let chatgpt = ConnectionId::new("chatgpt");
    app.connections = vec![ConnectionDescriptor {
        id: chatgpt.clone(),
        provider: "openai-chatgpt".to_owned(),
        base_url: "https://chatgpt.example.test/".parse().expect("url"),
        default_model: "gpt-5.4-mini".to_owned(),
        auth_methods: Vec::new(),
        auth_sources: Vec::new(),
        model_fallbacks: Vec::new(),
        discoverable_model_selectors: Vec::new(),
    }];
    app.connection_models = vec![ConnectionModelInventory {
        connection_id: chatgpt.clone(),
        provider: "openai-chatgpt".to_owned(),
        default_model: "gpt-5.4".to_owned(),
        auth_methods: Vec::new(),
        auth_state: ConnectionAuthState::Configured,
        auth_source: Some("oauth".to_owned()),
        support_state: ConnectionSupportState::RuntimeSupported,
        readiness_state: ConnectionReadinessState::Ready,
        probe_ready: true,
        probe_status: "healthy".to_owned(),
        discovered_source: Some("openai-chatgpt".to_owned()),
        models: vec![ConnectionModelOption {
            model_id: "gpt-5.4".to_owned(),
            source: ConnectionModelSource::Discovered,
        }],
    }];

    let now = OffsetDateTime::now_utc();
    let session = SessionRecord {
        session_id,
        project_root: "/tmp/belltower".into(),
        connection_id: chatgpt.clone(),
        model_id: Some("gpt-5.4-mini".to_owned()),
        tool_mode: SessionToolMode::Extended,
        settings_revision_id: 2,
        created_at: now,
        updated_at: now,
        status: SessionStatus::Active,
        display_name: None,
        objective: None,
        parent_session_id: None,
        parent_branch_id: None,
        parent_turn_id: None,
    };

    app.apply_session_metadata(session.clone());

    assert_eq!(app.connection_id, chatgpt);
    assert_eq!(app.effective_model(), "gpt-5.4-mini");
    assert_eq!(app.sessions, vec![session]);
}

fn event(seq_id: i64, payload: EventPayload) -> EventEnvelope {
    EventEnvelope {
        seq_id: Some(seq_id),
        event_id: bt_core::EventId::new(),
        session_id: SessionId::new(),
        branch_id: BranchId::new(),
        turn_id: None,
        span_id: SpanId::new(),
        parent_span_id: None,
        span_kind: SpanKind::Agent,
        occurred_at: OffsetDateTime::now_utc(),
        payload,
        attributes: Default::default(),
    }
}

fn assistant_message(text: &str) -> Message {
    Message {
        message_id: MessageId::new(),
        role: Role::Assistant,
        parts: vec![MessagePart::Text {
            text: text.to_owned(),
        }],
        created_at: OffsetDateTime::now_utc(),
    }
}

fn assistant_tool_call_message(
    tool_name: &str,
    call_id: &str,
    arguments: serde_json::Value,
) -> Message {
    Message {
        message_id: MessageId::new(),
        role: Role::Assistant,
        parts: vec![MessagePart::ToolCall {
            call: ToolCall {
                tool_name: tool_name.to_owned(),
                call_id: call_id.to_owned(),
                arguments,
            },
        }],
        created_at: OffsetDateTime::now_utc(),
    }
}

fn assistant_tool_result_message(
    tool_name: &str,
    call_id: &str,
    output: serde_json::Value,
) -> Message {
    Message {
        message_id: MessageId::new(),
        role: Role::Assistant,
        parts: vec![MessagePart::ToolResult {
            result: ToolResultEnvelope {
                call_id: ToolCallId::new(call_id),
                tool_name: tool_name.to_owned(),
                is_error: false,
                output,
                duration_ms: Some(5),
            },
        }],
        created_at: OffsetDateTime::now_utc(),
    }
}

fn tool_result_message(tool_name: &str, call_id: &str, output: serde_json::Value) -> Message {
    Message {
        message_id: MessageId::new(),
        role: Role::Tool,
        parts: vec![MessagePart::ToolResult {
            result: ToolResultEnvelope {
                call_id: ToolCallId::new(call_id),
                tool_name: tool_name.to_owned(),
                is_error: false,
                output,
                duration_ms: Some(5),
            },
        }],
        created_at: OffsetDateTime::now_utc(),
    }
}

fn lines_to_text(lines: &[ratatui::text::Line<'static>]) -> String {
    lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/operator")
}

fn assert_operator_fixture(name: &str, actual: &str) {
    let path = fixture_dir().join(name);
    if std::env::var_os("BLESS_TUI_FIXTURES").is_some() {
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture dir");
        fs::write(&path, actual).expect("write fixture");
        return;
    }

    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read fixture {}: {error}", path.display()));
    assert_eq!(actual, expected, "fixture mismatch: {}", path.display());
}

fn inspection_fixture_root() -> Value {
    serde_json::from_str(include_str!(
        "../../bt-protocol/tests/fixtures/inspection.json"
    ))
    .expect("inspection fixture json")
}

fn sample_health() -> HealthResponse {
    HealthResponse {
        status: "ok".to_owned(),
        protocol_version: "belltower.v1".to_owned(),
    }
}

fn sample_operator_surface_matrix() -> (
    StatusInspection,
    Vec<ConnectionModelInventory>,
    Vec<ModelBackendDescriptor>,
    bt_core::ModelRecommendationsReport,
    Vec<McpServerDescriptor>,
    Vec<McpToolDescriptor>,
) {
    let inspection = StatusInspection {
        auth_storage: "auto".to_owned(),
        default_connection: ConnectionId::new("openai"),
        connections: vec![
            ConnectionReadinessInspection {
                connection_id: ConnectionId::new("openai"),
                provider: "openai".to_owned(),
                default_model: "gpt-5.4-mini".to_owned(),
                auth_methods: Vec::new(),
                auth_state: ConnectionAuthState::Configured,
                auth_kind: Some(CredentialKind::ApiKey),
                auth_source: Some("keychain".to_owned()),
                auth_tried_sources: Vec::new(),
                support_state: ConnectionSupportState::RuntimeSupported,
                readiness_state: ConnectionReadinessState::Ready,
                probe_ready: true,
                probe_status: "ok".to_owned(),
            },
            ConnectionReadinessInspection {
                connection_id: ConnectionId::new("anthropic"),
                provider: "anthropic".to_owned(),
                default_model: "sonnet-4.6".to_owned(),
                auth_methods: Vec::new(),
                auth_state: ConnectionAuthState::Missing,
                auth_kind: None,
                auth_source: None,
                auth_tried_sources: vec!["env".to_owned(), "file".to_owned()],
                support_state: ConnectionSupportState::RuntimeSupported,
                readiness_state: ConnectionReadinessState::MissingAuth,
                probe_ready: false,
                probe_status: "login required".to_owned(),
            },
            ConnectionReadinessInspection {
                connection_id: ConnectionId::new("vertex"),
                provider: "vertex".to_owned(),
                default_model: "gemini-2.5-pro".to_owned(),
                auth_methods: Vec::new(),
                auth_state: ConnectionAuthState::Configured,
                auth_kind: Some(CredentialKind::JsonDocument),
                auth_source: Some("adc".to_owned()),
                auth_tried_sources: Vec::new(),
                support_state: ConnectionSupportState::Planned,
                readiness_state: ConnectionReadinessState::RuntimeUnsupported,
                probe_ready: false,
                probe_status: "provider planned".to_owned(),
            },
            ConnectionReadinessInspection {
                connection_id: ConnectionId::new("chatgpt"),
                provider: "openai-chatgpt".to_owned(),
                default_model: "gpt-5.4".to_owned(),
                auth_methods: Vec::new(),
                auth_state: ConnectionAuthState::Configured,
                auth_kind: Some(CredentialKind::OAuthToken),
                auth_source: Some("oauth".to_owned()),
                auth_tried_sources: vec!["keychain".to_owned()],
                support_state: ConnectionSupportState::RuntimeSupported,
                readiness_state: ConnectionReadinessState::ConfiguredModelUnavailable,
                probe_ready: false,
                probe_status: "use gpt-5.4-mini".to_owned(),
            },
        ],
    };

    let connections = vec![
        ConnectionModelInventory {
            connection_id: ConnectionId::new("openai"),
            provider: "openai".to_owned(),
            default_model: "gpt-5.4-mini".to_owned(),
            auth_methods: Vec::new(),
            auth_state: ConnectionAuthState::Configured,
            auth_source: Some("keychain".to_owned()),
            support_state: ConnectionSupportState::RuntimeSupported,
            readiness_state: ConnectionReadinessState::Ready,
            probe_ready: true,
            probe_status: "ok".to_owned(),
            discovered_source: Some("api".to_owned()),
            models: vec![
                ConnectionModelOption {
                    model_id: "gpt-5.4-mini".to_owned(),
                    source: ConnectionModelSource::Default,
                },
                ConnectionModelOption {
                    model_id: "gpt-5.4".to_owned(),
                    source: ConnectionModelSource::Discovered,
                },
            ],
        },
        ConnectionModelInventory {
            connection_id: ConnectionId::new("anthropic"),
            provider: "anthropic".to_owned(),
            default_model: "sonnet-4.6".to_owned(),
            auth_methods: Vec::new(),
            auth_state: ConnectionAuthState::Missing,
            auth_source: None,
            support_state: ConnectionSupportState::RuntimeSupported,
            readiness_state: ConnectionReadinessState::MissingAuth,
            probe_ready: false,
            probe_status: "login required".to_owned(),
            discovered_source: None,
            models: Vec::new(),
        },
        ConnectionModelInventory {
            connection_id: ConnectionId::new("vertex"),
            provider: "vertex".to_owned(),
            default_model: "gemini-2.5-pro".to_owned(),
            auth_methods: Vec::new(),
            auth_state: ConnectionAuthState::Configured,
            auth_source: Some("adc".to_owned()),
            support_state: ConnectionSupportState::Planned,
            readiness_state: ConnectionReadinessState::RuntimeUnsupported,
            probe_ready: false,
            probe_status: "provider planned".to_owned(),
            discovered_source: None,
            models: Vec::new(),
        },
        ConnectionModelInventory {
            connection_id: ConnectionId::new("chatgpt"),
            provider: "openai-chatgpt".to_owned(),
            default_model: "gpt-5.4".to_owned(),
            auth_methods: Vec::new(),
            auth_state: ConnectionAuthState::Configured,
            auth_source: Some("oauth".to_owned()),
            support_state: ConnectionSupportState::RuntimeSupported,
            readiness_state: ConnectionReadinessState::ConfiguredModelUnavailable,
            probe_ready: false,
            probe_status: "use gpt-5.4-mini".to_owned(),
            discovered_source: Some("api".to_owned()),
            models: vec![ConnectionModelOption {
                model_id: "gpt-5.4-mini".to_owned(),
                source: ConnectionModelSource::Discovered,
            }],
        },
    ];

    let backends = vec![ModelBackendDescriptor {
        kind: ModelBackendKind::Ollama,
        label: "Ollama".to_owned(),
        base_url: "http://127.0.0.1:11434/".parse().expect("ollama url"),
        status: ModelBackendStatus::Ready,
        available_models: vec!["qwen3.5:latest".to_owned(), "gpt-oss:20b".to_owned()],
    }];

    let report = bt_core::ModelRecommendationsReport {
        hardware: HardwareProfile {
            total_memory_gb: Some(64),
        },
        recommendations: vec![ModelRecommendation {
            label: "Laptop".to_owned(),
            max_memory_gb: 64,
            models: vec!["qwen3.5:latest".to_owned(), "gpt-oss:20b".to_owned()],
            fits_hardware: true,
        }],
    };

    let servers = vec![McpServerDescriptor {
        name: "docs".to_owned(),
        transport: bt_core::McpTransportKind::StreamableHttp,
        enabled: true,
        status: McpServerStatus::Degraded {
            reason: "retrying".to_owned(),
        },
    }];
    let tools = vec![McpToolDescriptor {
        server_name: "docs".to_owned(),
        tool_name: "search".to_owned(),
        qualified_name: "docs.search".to_owned(),
        description: "Search docs".to_owned(),
        parameters_schema: json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"]
        }),
    }];

    (inspection, connections, backends, report, servers, tools)
}

#[test]
fn session_error_event_prints_into_history_and_clears_task_spinner() {
    let mut app = test_app();
    app.set_task_status("Working", None);
    app.apply_stream_event(event(
        1,
        EventPayload::SessionError {
            class: bt_core::ErrorClass::Provider,
            code: "provider_error".to_owned(),
            message: "HTTP 400: unsupported reasoning_effort".to_owned(),
            retryable: false,
        },
    ));

    assert!(
        app.task_status.is_none(),
        "task spinner must clear when the session errors"
    );

    let mut committed = String::new();
    for _ in 0..4 {
        app.run_stream_commit_tick();
        if let Some(lines) = app.take_new_history_lines() {
            committed.push_str(&lines_to_text(&lines));
            committed.push('\n');
        }
    }
    assert!(
        committed.contains("unsupported reasoning_effort"),
        "session error text must land in visible history, got: {committed:?}"
    );
}

#[test]
fn streamed_assistant_text_commits_completed_lines_and_keeps_partial_tail_live() {
    let mut app = test_app();
    app.apply_stream_event(event(
        1,
        EventPayload::CompletionRequested {
            llm_call_ordinal: 0,
            provider: "openai".to_owned(),
            model: "o4-mini".to_owned(),
            message_count: 1,
        },
    ));
    app.apply_stream_event(event(
        2,
        EventPayload::CompletionChunk {
            llm_call_ordinal: Some(0),
            raw_chunk_index: None,
            deltas: vec![CompletionDelta::AppendText {
                text: "Hello\npartial tail".to_owned(),
            }],
        },
    ));

    app.run_stream_commit_tick();
    let committed = lines_to_text(
        &app.take_new_history_lines()
            .expect("completed line should commit to history"),
    );
    assert!(committed.contains("Hello"));

    let live = lines_to_text(&app.active_view_lines(80));
    assert!(live.contains("partial tail"));
}

#[test]
fn streamed_assistant_markdown_collapses_repeated_blank_lines() {
    let mut app = test_app();
    app.apply_stream_event(event(
        1,
        EventPayload::CompletionRequested {
            llm_call_ordinal: 0,
            provider: "openai".to_owned(),
            model: "o4-mini".to_owned(),
            message_count: 1,
        },
    ));
    app.apply_stream_event(event(
        2,
        EventPayload::CompletionChunk {
            llm_call_ordinal: Some(0),
            raw_chunk_index: None,
            deltas: vec![CompletionDelta::AppendText {
                text: "first\n\n\n\nsecond\n".to_owned(),
            }],
        },
    ));

    let mut committed_chunks = Vec::new();
    for _ in 0..4 {
        app.run_stream_commit_tick();
        if let Some(lines) = app.take_new_history_lines() {
            committed_chunks.push(lines_to_text(&lines));
        }
    }
    assert_eq!(committed_chunks.join("\n"), "first\n\nsecond");
}

#[test]
fn canonical_assistant_history_reflows_from_markdown_source() {
    let mut app = test_app();
    app.set_transcript_output_width(80);
    app.queue_scrollback_message(&assistant_message("alpha beta gamma delta"), Some(1));
    app.take_new_history_lines().expect("assistant history");

    app.set_transcript_output_width(12);
    let reset = lines_to_text(&app.compose_scrollback_reset_lines());

    assert!(reset.contains("alpha beta\ngamma delta"));
    assert!(!reset.contains("alpha beta gamma delta"));
}

#[test]
fn finalized_stream_history_consolidates_to_markdown_source_for_reset() {
    let mut app = test_app();
    app.set_transcript_output_width(80);
    app.apply_stream_event(event(
        1,
        EventPayload::CompletionRequested {
            llm_call_ordinal: 0,
            provider: "openai".to_owned(),
            model: "o4-mini".to_owned(),
            message_count: 1,
        },
    ));
    app.apply_stream_event(event(
        2,
        EventPayload::CompletionChunk {
            llm_call_ordinal: Some(0),
            raw_chunk_index: None,
            deltas: vec![CompletionDelta::AppendText {
                text: "alpha beta gamma delta\n".to_owned(),
            }],
        },
    ));

    app.run_stream_commit_tick();
    app.take_new_history_lines()
        .expect("streamed history should be inserted before finalization");
    app.apply_stream_event(event(
        3,
        EventPayload::CompletionFinished {
            llm_call_ordinal: 0,
            provider: "openai".to_owned(),
            model: "o4-mini".to_owned(),
            usage: TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
                cache_read_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
            },
            cost: None,
            finish_reason: "stop".to_owned(),
            latency_ms: 10,
        },
    ));

    app.set_transcript_output_width(12);
    let reset = lines_to_text(&app.compose_scrollback_reset_lines());

    assert!(reset.contains("alpha beta\ngamma delta"));
    assert!(!reset.contains("alpha beta gamma delta"));
}

#[test]
fn exploration_tool_stays_live_until_assistant_text_starts() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_list");
    app.apply_stream_event(event(
        1,
        EventPayload::ToolCallRequested {
            call_id: call_id.clone(),
            tool_name: "list".to_owned(),
            arguments: json!({ "path": "." }),
        },
    ));

    let live_before = lines_to_text(&app.active_view_lines(80));
    assert!(live_before.contains("Exploring"));
    assert!(live_before.contains("List ."));

    app.apply_stream_event(event(
        2,
        EventPayload::ToolExecutionFinished {
            call_id: call_id.clone(),
            tool_name: "list".to_owned(),
            result: ToolResultEnvelope {
                call_id,
                tool_name: "list".to_owned(),
                is_error: false,
                output: json!({ "entries": ["Cargo.toml", "src"] }),
                duration_ms: Some(12),
            },
        },
    ));

    assert!(
        app.take_new_history_lines().is_none(),
        "completed exploration should stay in the live cell until answer text starts"
    );
    let live_after = lines_to_text(&app.active_view_lines(80));
    assert!(live_after.contains("Explored"));
    assert!(!live_after.contains("Exploring"));

    app.apply_stream_event(event(
        3,
        EventPayload::CompletionChunk {
            llm_call_ordinal: Some(1),
            raw_chunk_index: None,
            deltas: vec![CompletionDelta::AppendText {
                text: "Done.\n".to_owned(),
            }],
        },
    ));

    let committed = lines_to_text(
        &app.take_new_history_lines()
            .expect("completed exploration should commit before assistant text"),
    );
    assert_eq!(committed.matches("Explored").count(), 1);
    assert!(committed.contains("List ."));
    assert!(committed.contains("─"));

    app.run_stream_commit_tick();
    let assistant = lines_to_text(
        &app.take_new_history_lines()
            .expect("assistant text should commit after the work separator"),
    );
    assert!(assistant.contains("Done."));
}

#[test]
fn canonical_tool_call_message_reconciles_live_tool_without_duplicate_history() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_list");
    app.apply_stream_event(event(
        1,
        EventPayload::ToolCallRequested {
            call_id: call_id.clone(),
            tool_name: "list".to_owned(),
            arguments: json!({ "path": "." }),
        },
    ));

    app.apply_stream_event(event(
        2,
        EventPayload::MessageAppended {
            message: assistant_tool_call_message(
                "list",
                &call_id.to_string(),
                json!({ "path": "." }),
            ),
        },
    ));

    assert!(
        app.take_new_history_lines().is_none(),
        "canonical tool call should reconcile the live tool state instead of printing history"
    );
    let live = lines_to_text(&app.active_view_lines(80));
    assert_eq!(live.matches("Exploring").count(), 1);
    assert_eq!(live.matches("List .").count(), 1);

    app.apply_stream_event(event(
        3,
        EventPayload::ToolExecutionFinished {
            call_id: call_id.clone(),
            tool_name: "list".to_owned(),
            result: ToolResultEnvelope {
                call_id,
                tool_name: "list".to_owned(),
                is_error: false,
                output: json!({ "entries": ["Cargo.toml", "src"] }),
                duration_ms: Some(12),
            },
        },
    ));

    assert!(
        app.take_new_history_lines().is_none(),
        "completed exploration should remain live until assistant text starts"
    );
    let live_after = lines_to_text(&app.active_view_lines(80));
    assert_eq!(live_after.matches("Explored").count(), 1);
    assert_eq!(live_after.matches("List .").count(), 1);
}

#[test]
fn canonical_tool_messages_do_not_duplicate_completed_live_tool() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_list");
    app.apply_stream_event(event(
        1,
        EventPayload::ToolCallRequested {
            call_id: call_id.clone(),
            tool_name: "list".to_owned(),
            arguments: json!({ "path": "." }),
        },
    ));
    app.apply_stream_event(event(
        2,
        EventPayload::ToolExecutionFinished {
            call_id: call_id.clone(),
            tool_name: "list".to_owned(),
            result: ToolResultEnvelope {
                call_id: call_id.clone(),
                tool_name: "list".to_owned(),
                is_error: false,
                output: json!({ "entries": ["Cargo.toml", "src"] }),
                duration_ms: Some(12),
            },
        },
    ));

    assert!(
        app.take_new_history_lines().is_none(),
        "completed exploration should still be live"
    );

    app.apply_stream_event(event(
        3,
        EventPayload::MessageAppended {
            message: assistant_tool_call_message(
                "list",
                &call_id.to_string(),
                json!({ "path": "." }),
            ),
        },
    ));

    app.apply_stream_event(event(
        4,
        EventPayload::MessageAppended {
            message: assistant_tool_result_message(
                "list",
                &call_id.to_string(),
                json!({ "entries": ["Cargo.toml", "src"] }),
            ),
        },
    ));

    assert!(
        app.take_new_history_lines().is_none(),
        "canonical tool result should reconcile with the completed live cell"
    );
    let live = lines_to_text(&app.active_view_lines(80));
    assert_eq!(live.matches("Explored").count(), 1);
    assert_eq!(live.matches("List .").count(), 1);

    app.apply_stream_event(event(
        5,
        EventPayload::CompletionChunk {
            llm_call_ordinal: Some(1),
            raw_chunk_index: None,
            deltas: vec![CompletionDelta::AppendText {
                text: "Done.\n".to_owned(),
            }],
        },
    ));

    let committed = lines_to_text(
        &app.take_new_history_lines()
            .expect("completed live tool should commit once when answer starts"),
    );
    assert_eq!(committed.matches("Explored").count(), 1);
    assert_eq!(committed.matches("List .").count(), 1);
}

#[test]
fn latest_transcript_page_tool_messages_reconcile_with_live_exploration() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_list");
    app.apply_stream_event(event(
        1,
        EventPayload::ToolCallRequested {
            call_id: call_id.clone(),
            tool_name: "list".to_owned(),
            arguments: json!({ "path": "." }),
        },
    ));
    let _ = app.take_new_history_lines();

    app.merge_latest_transcript_page(LoadedTranscriptPage {
        messages: vec![
            assistant_tool_call_message("list", &call_id.to_string(), json!({ "path": "." })),
            assistant_tool_result_message(
                "list",
                &call_id.to_string(),
                json!({ "entries": ["Cargo.toml", "src"] }),
            ),
        ],
        message_seq_ids: vec![Some(2), Some(3)],
        operator_commands: Vec::new(),
        has_more_before: false,
        last_event_id: Some(3),
    });

    assert!(
        app.take_new_history_lines().is_none(),
        "transcript backfill must not print canonical tool messages while the live cell owns them"
    );
    let live = lines_to_text(&app.active_view_lines(80));
    assert_eq!(live.matches("Explored").count(), 1);
    assert_eq!(live.matches("List .").count(), 1);

    app.apply_stream_event(event(
        4,
        EventPayload::CompletionChunk {
            llm_call_ordinal: Some(1),
            raw_chunk_index: None,
            deltas: vec![CompletionDelta::AppendText {
                text: "Done.\n".to_owned(),
            }],
        },
    ));

    let committed = lines_to_text(
        &app.take_new_history_lines()
            .expect("completed live cell should commit once when assistant text starts"),
    );
    assert_eq!(committed.matches("Explored").count(), 1);
    assert_eq!(committed.matches("List .").count(), 1);
}

#[test]
fn latest_transcript_page_flushes_live_tool_before_loaded_assistant_text() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_list");
    app.apply_stream_event(event(
        1,
        EventPayload::ToolCallRequested {
            call_id: call_id.clone(),
            tool_name: "list".to_owned(),
            arguments: json!({ "path": "." }),
        },
    ));
    let _ = app.take_new_history_lines();

    app.merge_latest_transcript_page(LoadedTranscriptPage {
        messages: vec![
            assistant_tool_call_message("list", &call_id.to_string(), json!({ "path": "." })),
            assistant_tool_result_message(
                "list",
                &call_id.to_string(),
                json!({ "entries": ["Cargo.toml", "src"] }),
            ),
            assistant_message("Done."),
        ],
        message_seq_ids: vec![Some(2), Some(3), Some(4)],
        operator_commands: Vec::new(),
        has_more_before: false,
        last_event_id: Some(4),
    });

    let committed = lines_to_text(
        &app.take_new_history_lines()
            .expect("page merge should flush live work before loaded assistant text"),
    );
    let explored = committed.find("Explored").expect("exploration block");
    let separator = committed.find("─").expect("work separator");
    let assistant = committed.find("Done.").expect("assistant text");
    assert!(explored < separator);
    assert!(separator < assistant);
    assert_eq!(committed.matches("Explored").count(), 1);
    assert_eq!(committed.matches("Done.").count(), 1);
}

#[test]
fn transcript_page_loads_do_not_regress_stream_cursor() {
    let mut app = test_app();
    app.last_event_id = Some(10);

    app.merge_latest_transcript_page(LoadedTranscriptPage {
        messages: vec![assistant_message("Earlier answer.")],
        message_seq_ids: vec![Some(4)],
        operator_commands: Vec::new(),
        has_more_before: true,
        last_event_id: Some(4),
    });
    assert_eq!(app.last_event_id, Some(10));

    app.prepend_older_transcript_page(LoadedTranscriptPage {
        messages: vec![assistant_message("Oldest answer.")],
        message_seq_ids: vec![Some(2)],
        operator_commands: Vec::new(),
        has_more_before: false,
        last_event_id: Some(2),
    });
    assert_eq!(app.last_event_id, Some(10));

    app.merge_latest_transcript_page(LoadedTranscriptPage {
        messages: vec![assistant_message("Newer answer.")],
        message_seq_ids: vec![Some(12)],
        operator_commands: Vec::new(),
        has_more_before: false,
        last_event_id: Some(12),
    });
    assert_eq!(app.last_event_id, Some(12));
}

#[test]
fn late_canonical_tool_messages_do_not_reprint_flushed_live_history() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_list");

    app.apply_stream_event(event(
        1,
        EventPayload::MessageAppended {
            message: Message::text(Role::User, "summarize this project"),
        },
    ));
    let _ = app.take_new_history_lines();

    app.apply_stream_event(event(
        2,
        EventPayload::ToolCallRequested {
            call_id: call_id.clone(),
            tool_name: "list".to_owned(),
            arguments: json!({ "path": "." }),
        },
    ));
    app.apply_stream_event(event(
        3,
        EventPayload::ToolExecutionFinished {
            call_id: call_id.clone(),
            tool_name: "list".to_owned(),
            result: ToolResultEnvelope {
                call_id: call_id.clone(),
                tool_name: "list".to_owned(),
                is_error: false,
                output: json!({ "entries": ["Cargo.toml", "src"] }),
                duration_ms: Some(12),
            },
        },
    ));
    app.apply_stream_event(event(
        4,
        EventPayload::CompletionChunk {
            llm_call_ordinal: Some(1),
            raw_chunk_index: None,
            deltas: vec![CompletionDelta::AppendText {
                text: "Done.\n".to_owned(),
            }],
        },
    ));

    let live_commit = lines_to_text(
        &app.take_new_history_lines()
            .expect("live tool history should flush before assistant text"),
    );
    assert_eq!(live_commit.matches("Explored").count(), 1);
    assert_eq!(live_commit.matches("List .").count(), 1);
    app.run_stream_commit_tick();
    let _ = app.take_new_history_lines();

    app.merge_latest_transcript_page(LoadedTranscriptPage {
        messages: vec![
            assistant_tool_call_message("list", &call_id.to_string(), json!({ "path": "." })),
            assistant_tool_result_message(
                "list",
                &call_id.to_string(),
                json!({ "entries": ["Cargo.toml", "src"] }),
            ),
        ],
        message_seq_ids: vec![Some(5), Some(6)],
        operator_commands: Vec::new(),
        has_more_before: false,
        last_event_id: Some(6),
    });

    assert!(
        app.take_new_history_lines().is_none(),
        "late canonical transcript entries for a flushed active tool must not duplicate history"
    );
    assert!(
        !lines_to_text(&app.active_view_lines(80)).contains("Explored"),
        "late canonical tool messages must not recreate a live tool cell"
    );
}

#[test]
fn final_work_separator_waits_until_active_cells_flush() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_list");

    app.apply_stream_event(event(
        1,
        EventPayload::ToolCallRequested {
            call_id: call_id.clone(),
            tool_name: "list".to_owned(),
            arguments: json!({ "path": "." }),
        },
    ));
    app.apply_stream_event(event(
        2,
        EventPayload::ToolExecutionFinished {
            call_id,
            tool_name: "list".to_owned(),
            result: ToolResultEnvelope {
                call_id: ToolCallId::new("call_list"),
                tool_name: "list".to_owned(),
                is_error: false,
                output: json!({ "entries": ["Cargo.toml", "src"] }),
                duration_ms: Some(12),
            },
        },
    ));

    assert!(
        !app.emit_final_work_separator_if_needed(),
        "separator should not commit while the completed exploration cell is still live"
    );
    assert!(app.take_new_history_lines().is_none());
    assert!(lines_to_text(&app.active_view_lines(80)).contains("Explored"));
}

#[test]
fn provider_completion_keeps_pending_tool_live_until_execution_result() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_shell");

    app.apply_stream_event(event(
        1,
        EventPayload::ToolCallRequested {
            call_id: call_id.clone(),
            tool_name: "shell".to_owned(),
            arguments: json!({ "command": "printf tui-approval" }),
        },
    ));
    app.apply_stream_event(event(
        2,
        EventPayload::CompletionFinished {
            llm_call_ordinal: 1,
            provider: "openai".to_owned(),
            model: "o4-mini".to_owned(),
            usage: TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
                cache_read_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
            },
            cost: None,
            finish_reason: "tool_calls".to_owned(),
            latency_ms: 10,
        },
    ));

    assert!(
        app.take_new_history_lines().is_none(),
        "provider completion must not commit an incomplete tool row"
    );
    assert!(
        lines_to_text(&app.active_view_lines(80)).contains("Calling shell"),
        "unresolved generic tools should remain visible in the live tool surface"
    );

    app.apply_stream_event(event(
        3,
        EventPayload::TurnFinished {
            turn_id: bt_core::TurnId::new(),
            provider: "openai".to_owned(),
            model: "o4-mini".to_owned(),
            status: "waiting_on_approval".to_owned(),
            finish_reason: Some("tool_calls".to_owned()),
            latency_ms: 10,
        },
    ));
    assert!(
        app.take_new_history_lines().is_none(),
        "turn completion must not commit an unresolved generic tool row"
    );

    app.apply_stream_event(event(
        4,
        EventPayload::ToolExecutionFinished {
            call_id: call_id.clone(),
            tool_name: "shell".to_owned(),
            result: ToolResultEnvelope {
                call_id,
                tool_name: "shell".to_owned(),
                is_error: false,
                output: json!({ "status": 0 }),
                duration_ms: Some(12),
            },
        },
    ));

    let committed = lines_to_text(
        &app.take_new_history_lines()
            .expect("resolved non-exploration tool should commit once"),
    );
    assert_eq!(committed.matches("Calling shell").count(), 0);
    assert_eq!(committed.matches("Called shell").count(), 1);
}

#[test]
fn write_tool_calls_render_live_and_commit_sequentially() {
    let mut app = test_app();
    let first = ToolCallId::new("call_write_a");
    let second = ToolCallId::new("call_write_b");

    app.apply_stream_event(event(
        1,
        EventPayload::ToolCallRequested {
            call_id: first.clone(),
            tool_name: "write".to_owned(),
            arguments: json!({ "path": "a.txt" }),
        },
    ));

    assert!(app.take_new_history_lines().is_none());
    let live_one = lines_to_text(&app.active_view_lines(80));
    assert!(live_one.contains("Calling write a.txt"));

    app.apply_stream_event(event(
        2,
        EventPayload::ToolCallRequested {
            call_id: second.clone(),
            tool_name: "write".to_owned(),
            arguments: json!({ "path": "b.txt" }),
        },
    ));

    let live_two = lines_to_text(&app.active_view_lines(80));
    assert_eq!(live_two.matches("Calling write").count(), 2);
    assert!(live_two.contains("a.txt"));
    assert!(live_two.contains("b.txt"));

    app.apply_stream_event(event(
        3,
        EventPayload::ToolExecutionFinished {
            call_id: first.clone(),
            tool_name: "write".to_owned(),
            result: ToolResultEnvelope {
                call_id: first,
                tool_name: "write".to_owned(),
                is_error: false,
                output: json!({ "path": "a.txt" }),
                duration_ms: Some(5),
            },
        },
    ));

    let committed_first = lines_to_text(
        &app.take_new_history_lines()
            .expect("first completed write should commit"),
    );
    assert!(committed_first.contains("Called write a.txt"));
    assert!(!committed_first.contains("b.txt"));
    let live_after_first = lines_to_text(&app.active_view_lines(80));
    assert!(live_after_first.contains("Calling write b.txt"));
    assert!(!live_after_first.contains("a.txt"));

    app.apply_stream_event(event(
        4,
        EventPayload::ToolExecutionFinished {
            call_id: second.clone(),
            tool_name: "write".to_owned(),
            result: ToolResultEnvelope {
                call_id: second,
                tool_name: "write".to_owned(),
                is_error: false,
                output: json!({ "path": "b.txt" }),
                duration_ms: Some(6),
            },
        },
    ));

    let committed_second = lines_to_text(
        &app.take_new_history_lines()
            .expect("second completed write should commit"),
    );
    assert!(committed_second.contains("Called write b.txt"));
    assert!(!committed_second.contains("a.txt"));
}

#[test]
fn session_readiness_summary_uses_selected_discovered_model() {
    let connection_id = ConnectionId::new("chatgpt");
    let inspection = StatusInspection {
        auth_storage: "auth store (/tmp/auth.json)".to_owned(),
        default_connection: connection_id.clone(),
        connections: vec![ConnectionReadinessInspection {
            connection_id: connection_id.clone(),
            provider: "openai-chatgpt".to_owned(),
            default_model: "gpt-5.4".to_owned(),
            auth_methods: Vec::new(),
            auth_state: ConnectionAuthState::Configured,
            auth_kind: Some(CredentialKind::OAuthToken),
            auth_source: Some("auth store (/tmp/auth.json, literal)".to_owned()),
            auth_tried_sources: Vec::new(),
            support_state: ConnectionSupportState::RuntimeSupported,
            readiness_state: ConnectionReadinessState::ConfiguredModelUnavailable,
            probe_ready: false,
            probe_status: "healthy via oauth token from auth store (/tmp/auth.json, literal), but configured model `gpt-5.4` is not available on openai-chatgpt".to_owned(),
        }],
    };
    let inventories = vec![ConnectionModelInventory {
        connection_id: connection_id.clone(),
        provider: "openai-chatgpt".to_owned(),
        default_model: "gpt-5.4".to_owned(),
        auth_methods: Vec::new(),
        auth_state: ConnectionAuthState::Configured,
        auth_source: Some("auth store (/tmp/auth.json, literal)".to_owned()),
        support_state: ConnectionSupportState::RuntimeSupported,
        readiness_state: ConnectionReadinessState::ConfiguredModelUnavailable,
        probe_ready: false,
        probe_status: "healthy via oauth token from auth store (/tmp/auth.json, literal), but configured model `gpt-5.4` is not available on openai-chatgpt".to_owned(),
        discovered_source: Some("openai-chatgpt".to_owned()),
        models: vec![
            ConnectionModelOption {
                model_id: "gpt-5.4".to_owned(),
                source: ConnectionModelSource::Default,
            },
            ConnectionModelOption {
                model_id: "gpt-5.4-pro".to_owned(),
                source: ConnectionModelSource::Discovered,
            },
        ],
    }];

    assert_eq!(
        session_readiness_summary(
            Some(&inspection),
            &inventories,
            &connection_id,
            "gpt-5.4-pro"
        ),
        "ready: healthy via oauth token from auth store (/tmp/auth.json, literal)"
    );
}

#[test]
fn session_readiness_summary_reports_selected_model_when_not_discovered() {
    let connection_id = ConnectionId::new("chatgpt");
    let inspection = StatusInspection {
        auth_storage: "auth store (/tmp/auth.json)".to_owned(),
        default_connection: connection_id.clone(),
        connections: vec![ConnectionReadinessInspection {
            connection_id: connection_id.clone(),
            provider: "openai-chatgpt".to_owned(),
            default_model: "gpt-5.4".to_owned(),
            auth_methods: Vec::new(),
            auth_state: ConnectionAuthState::Configured,
            auth_kind: Some(CredentialKind::OAuthToken),
            auth_source: Some("auth store (/tmp/auth.json, literal)".to_owned()),
            auth_tried_sources: Vec::new(),
            support_state: ConnectionSupportState::RuntimeSupported,
            readiness_state: ConnectionReadinessState::Ready,
            probe_ready: true,
            probe_status: "healthy via oauth token from auth store (/tmp/auth.json, literal)"
                .to_owned(),
        }],
    };
    let inventories = vec![ConnectionModelInventory {
        connection_id: connection_id.clone(),
        provider: "openai-chatgpt".to_owned(),
        default_model: "gpt-5.4".to_owned(),
        auth_methods: Vec::new(),
        auth_state: ConnectionAuthState::Configured,
        auth_source: Some("auth store (/tmp/auth.json, literal)".to_owned()),
        support_state: ConnectionSupportState::RuntimeSupported,
        readiness_state: ConnectionReadinessState::Ready,
        probe_ready: true,
        probe_status: "healthy via oauth token from auth store (/tmp/auth.json, literal)"
            .to_owned(),
        discovered_source: Some("openai-chatgpt".to_owned()),
        models: vec![ConnectionModelOption {
            model_id: "gpt-5.4-pro".to_owned(),
            source: ConnectionModelSource::Discovered,
        }],
    }];

    assert_eq!(
        session_readiness_summary(Some(&inspection), &inventories, &connection_id, "gpt-5.4"),
        "configured model unavailable: healthy via oauth token from auth store (/tmp/auth.json, literal), but current session model `gpt-5.4` is not available on openai-chatgpt"
    );
}

#[test]
fn persisted_default_model_must_be_discoverable() {
    let mut app = test_app();
    let connection_id = ConnectionId::new("chatgpt");
    app.connection_models = vec![ConnectionModelInventory {
        connection_id: connection_id.clone(),
        provider: "openai-chatgpt".to_owned(),
        default_model: "gpt-5.4".to_owned(),
        auth_methods: Vec::new(),
        auth_state: ConnectionAuthState::Configured,
        auth_source: Some("auth store (/tmp/auth.json, literal)".to_owned()),
        support_state: ConnectionSupportState::RuntimeSupported,
        readiness_state: ConnectionReadinessState::ConfiguredModelUnavailable,
        probe_ready: false,
        probe_status: "configured model unavailable".to_owned(),
        discovered_source: Some("openai-chatgpt".to_owned()),
        models: vec![ConnectionModelOption {
            model_id: "gpt-5.4-mini".to_owned(),
            source: ConnectionModelSource::Discovered,
        }],
    }];

    assert!(
        app.validate_persisted_default_model(&connection_id, "gpt-5.4-mini")
            .is_ok()
    );
    assert_eq!(
        app.validate_persisted_default_model(&connection_id, "gpt-5.4")
            .expect_err("non-discoverable default should be rejected"),
        "Default model `gpt-5.4` is not discoverable for `chatgpt`; choose one of: gpt-5.4-mini"
    );
}

#[test]
fn tool_block_commits_before_later_summary_text_streams() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_readme");
    app.apply_stream_event(event(
        1,
        EventPayload::ToolCallRequested {
            call_id: call_id.clone(),
            tool_name: "read".to_owned(),
            arguments: json!({ "path": "README.md" }),
        },
    ));
    app.apply_stream_event(event(
        2,
        EventPayload::ToolExecutionFinished {
            call_id: call_id.clone(),
            tool_name: "read".to_owned(),
            result: ToolResultEnvelope {
                call_id,
                tool_name: "read".to_owned(),
                is_error: false,
                output: json!({ "path": "README.md" }),
                duration_ms: Some(15),
            },
        },
    ));
    assert!(
        app.take_new_history_lines().is_none(),
        "completed exploration should stay live until assistant text starts"
    );

    app.apply_stream_event(event(
        3,
        EventPayload::CompletionChunk {
            llm_call_ordinal: Some(1),
            raw_chunk_index: None,
            deltas: vec![CompletionDelta::AppendText {
                text: "Here is the summary.\n".to_owned(),
            }],
        },
    ));

    let tool_lines = lines_to_text(
        &app.take_new_history_lines()
            .expect("tool block should commit before assistant text"),
    );
    assert!(tool_lines.contains("Explored"));
    assert!(tool_lines.contains("Read README.md"));
    assert!(tool_lines.contains("─"));

    app.run_stream_commit_tick();
    let summary = lines_to_text(
        &app.take_new_history_lines()
            .expect("summary text should commit after the tool block"),
    );
    assert!(summary.contains("Here is the summary."));
}

#[test]
fn canonical_assistant_message_does_not_reprint_already_streamed_text() {
    let mut app = test_app();
    app.apply_stream_event(event(
        1,
        EventPayload::CompletionChunk {
            llm_call_ordinal: Some(1),
            raw_chunk_index: None,
            deltas: vec![CompletionDelta::AppendText {
                text: "One line.\n".to_owned(),
            }],
        },
    ));
    app.run_stream_commit_tick();
    let first = lines_to_text(
        &app.take_new_history_lines()
            .expect("streamed line should commit once"),
    );
    assert!(first.contains("One line."));

    app.apply_stream_event(event(
        2,
        EventPayload::MessageAppended {
            message: assistant_message("One line."),
        },
    ));
    assert!(app.take_new_history_lines().is_none());
}

#[test]
fn canonical_assistant_message_mismatch_commits_final_text() {
    let mut app = test_app();
    app.apply_stream_event(event(
        1,
        EventPayload::CompletionChunk {
            llm_call_ordinal: Some(1),
            raw_chunk_index: None,
            deltas: vec![CompletionDelta::AppendText {
                text: "draft text".to_owned(),
            }],
        },
    ));
    app.run_stream_commit_tick();
    let _ = app.take_new_history_lines();

    app.apply_stream_event(event(
        2,
        EventPayload::MessageAppended {
            message: assistant_message("final canonical text"),
        },
    ));

    let committed = lines_to_text(
        &app.take_new_history_lines()
            .expect("mismatched final assistant text should not be dropped"),
    );
    assert!(committed.contains("final canonical text"));
}

#[test]
fn pure_conversational_turn_does_not_emit_final_work_separator() {
    let mut app = test_app();
    app.apply_stream_event(event(
        1,
        EventPayload::CompletionRequested {
            llm_call_ordinal: 0,
            provider: "openai".to_owned(),
            model: "o4-mini".to_owned(),
            message_count: 1,
        },
    ));
    app.apply_stream_event(event(
        2,
        EventPayload::CompletionChunk {
            llm_call_ordinal: Some(0),
            raw_chunk_index: None,
            deltas: vec![CompletionDelta::AppendText {
                text: "Hello there.\n".to_owned(),
            }],
        },
    ));
    app.run_stream_commit_tick();
    let committed = lines_to_text(
        &app.take_new_history_lines()
            .expect("assistant line should commit to history"),
    );
    assert!(committed.contains("Hello there."));
    assert!(!committed.contains("─"));

    app.apply_stream_event(event(
        3,
        EventPayload::TurnFinished {
            turn_id: bt_core::TurnId::new(),
            provider: "openai".to_owned(),
            model: "o4-mini".to_owned(),
            status: "completed".to_owned(),
            finish_reason: Some("stop".to_owned()),
            latency_ms: 10,
        },
    ));

    let trailing = app
        .take_new_history_lines()
        .map(|lines| lines_to_text(&lines));
    assert!(trailing.as_deref().is_none_or(|text| !text.contains("─")));
}

#[test]
fn tool_only_turn_emits_final_work_separator_on_completion() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_list");

    app.apply_stream_event(event(
        1,
        EventPayload::ToolCallRequested {
            call_id: call_id.clone(),
            tool_name: "list".to_owned(),
            arguments: json!({ "path": "." }),
        },
    ));
    app.apply_stream_event(event(
        2,
        EventPayload::ToolExecutionFinished {
            call_id: call_id.clone(),
            tool_name: "list".to_owned(),
            result: ToolResultEnvelope {
                call_id,
                tool_name: "list".to_owned(),
                is_error: false,
                output: json!({ "entries": ["Cargo.toml", "src"] }),
                duration_ms: Some(12),
            },
        },
    ));
    assert!(
        app.take_new_history_lines().is_none(),
        "completed exploration should stay live until the turn ends or answer text starts"
    );

    app.apply_stream_event(event(
        3,
        EventPayload::TurnFinished {
            turn_id: bt_core::TurnId::new(),
            provider: "openai".to_owned(),
            model: "o4-mini".to_owned(),
            status: "completed".to_owned(),
            finish_reason: Some("tool_done".to_owned()),
            latency_ms: 12,
        },
    ));

    let trailing_lines = app
        .take_new_history_lines()
        .expect("turn completion should flush live exploration and emit separator");
    assert!(
        !trailing_lines
            .first()
            .is_some_and(|line| line.to_string().trim().is_empty()),
        "work separator should not add an empty paragraph gap before itself"
    );
    let trailing = lines_to_text(&trailing_lines);
    assert!(trailing.contains("Explored"));
    assert!(trailing.contains("List ."));
    assert!(trailing.contains("─"));
}

#[test]
fn canonical_assistant_summary_waits_for_tool_and_only_emits_unseen_suffix() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_list");

    app.apply_stream_event(event(
        1,
        EventPayload::CompletionChunk {
            llm_call_ordinal: Some(1),
            raw_chunk_index: None,
            deltas: vec![CompletionDelta::AppendText {
                text: "Intro line.\n".to_owned(),
            }],
        },
    ));
    app.run_stream_commit_tick();
    let intro = lines_to_text(
        &app.take_new_history_lines()
            .expect("intro line should commit once"),
    );
    assert!(intro.contains("Intro line."));

    app.apply_stream_event(event(
        2,
        EventPayload::ToolCallRequested {
            call_id: call_id.clone(),
            tool_name: "list".to_owned(),
            arguments: json!({ "path": "." }),
        },
    ));

    app.apply_stream_event(event(
        3,
        EventPayload::MessageAppended {
            message: assistant_message("Intro line.\nSummary after tool."),
        },
    ));
    assert!(app.take_new_history_lines().is_none());

    app.apply_stream_event(event(
        4,
        EventPayload::ToolExecutionFinished {
            call_id: call_id.clone(),
            tool_name: "list".to_owned(),
            result: ToolResultEnvelope {
                call_id,
                tool_name: "list".to_owned(),
                is_error: false,
                output: json!({ "entries": ["Cargo.toml", "src"] }),
                duration_ms: Some(7),
            },
        },
    ));

    let committed = lines_to_text(
        &app.take_new_history_lines()
            .expect("tool block and suffix should commit after tool completion"),
    );
    assert!(committed.contains("Explored"));
    assert!(committed.contains("List ."));
    assert!(committed.contains("Summary after tool."));
    assert_eq!(committed.matches("Intro line.").count(), 0);
    assert!(committed.find("Explored").unwrap() < committed.find("Summary after tool.").unwrap());
}

#[test]
fn streamed_assistant_lines_do_not_commit_ahead_of_live_tool_cells() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_list");

    app.apply_stream_event(event(
        1,
        EventPayload::ToolCallRequested {
            call_id: call_id.clone(),
            tool_name: "list".to_owned(),
            arguments: json!({ "path": "." }),
        },
    ));

    app.apply_stream_event(event(
        2,
        EventPayload::CompletionChunk {
            llm_call_ordinal: Some(1),
            raw_chunk_index: None,
            deltas: vec![CompletionDelta::AppendText {
                text: "Summary after tool.\n".to_owned(),
            }],
        },
    ));

    app.run_stream_commit_tick();
    assert!(
        app.take_new_history_lines().is_none(),
        "assistant text should stay live while the tool cell is unresolved"
    );
    let live = lines_to_text(&app.active_view_lines(80));
    assert!(live.contains("Exploring"));
    assert!(live.contains("List ."));
    assert!(!live.contains("Summary after tool."));

    app.apply_stream_event(event(
        3,
        EventPayload::ToolExecutionFinished {
            call_id: call_id.clone(),
            tool_name: "list".to_owned(),
            result: ToolResultEnvelope {
                call_id,
                tool_name: "list".to_owned(),
                is_error: false,
                output: json!({ "entries": ["Cargo.toml", "src"] }),
                duration_ms: Some(7),
            },
        },
    ));

    let committed_tool = lines_to_text(
        &app.take_new_history_lines()
            .expect("completed tool block should commit first"),
    );
    assert!(committed_tool.contains("Explored"));
    assert!(committed_tool.contains("List ."));
    assert!(!committed_tool.contains("Summary after tool."));

    app.run_stream_commit_tick();
    let committed_summary = lines_to_text(
        &app.take_new_history_lines()
            .expect("assistant text should commit after the tool batch"),
    );
    assert!(committed_summary.contains("Summary after tool."));
}

#[test]
fn grouped_exploration_commits_after_all_grouped_tools_finish() {
    let mut app = test_app();
    let first = ToolCallId::new("call_list");
    let second = ToolCallId::new("call_read");

    app.apply_stream_event(event(
        1,
        EventPayload::ToolCallRequested {
            call_id: first.clone(),
            tool_name: "list".to_owned(),
            arguments: json!({ "path": "." }),
        },
    ));
    app.apply_stream_event(event(
        2,
        EventPayload::ToolCallRequested {
            call_id: second.clone(),
            tool_name: "read".to_owned(),
            arguments: json!({ "path": "README.md" }),
        },
    ));

    app.apply_stream_event(event(
        3,
        EventPayload::ToolExecutionFinished {
            call_id: first.clone(),
            tool_name: "list".to_owned(),
            result: ToolResultEnvelope {
                call_id: first,
                tool_name: "list".to_owned(),
                is_error: false,
                output: json!({ "entries": ["Cargo.toml", "README.md"] }),
                duration_ms: Some(7),
            },
        },
    ));

    assert!(
        app.take_new_history_lines().is_none(),
        "Codex-style exploration groups stay live until all grouped calls finish"
    );
    let live = lines_to_text(&app.active_view_lines(80));
    assert!(live.contains("Exploring"));
    assert!(live.contains("List ."));
    assert!(live.contains("Read README.md"));

    app.apply_stream_event(event(
        4,
        EventPayload::CompletionChunk {
            llm_call_ordinal: Some(1),
            raw_chunk_index: None,
            deltas: vec![CompletionDelta::AppendText {
                text: "Summary after tools.\n".to_owned(),
            }],
        },
    ));
    app.run_stream_commit_tick();
    assert!(
        app.take_new_history_lines().is_none(),
        "assistant text should still wait behind the unresolved later tool"
    );

    app.apply_stream_event(event(
        5,
        EventPayload::ToolExecutionFinished {
            call_id: second.clone(),
            tool_name: "read".to_owned(),
            result: ToolResultEnvelope {
                call_id: second,
                tool_name: "read".to_owned(),
                is_error: false,
                output: json!({ "path": "README.md" }),
                duration_ms: Some(9),
            },
        },
    ));

    let committed_group = lines_to_text(
        &app.take_new_history_lines()
            .expect("completed exploration group should commit before assistant text"),
    );
    assert!(committed_group.contains("Explored"));
    assert!(committed_group.contains("List ."));
    assert!(committed_group.contains("Read README.md"));

    app.run_stream_commit_tick();
    let committed_summary = lines_to_text(
        &app.take_new_history_lines()
            .expect("assistant text should commit after all live tools flush"),
    );
    assert!(committed_summary.contains("Summary after tools."));
}

#[test]
fn compose_scrollback_reset_lines_includes_banner_and_committed_history() {
    let mut app = test_app();
    app.apply_stream_event(event(
        1,
        EventPayload::MessageAppended {
            message: Message::text(Role::User, "hello"),
        },
    ));
    let reset = lines_to_text(&app.compose_scrollback_reset_lines());
    assert!(reset.contains("Welcome to Belltower"));
    assert!(reset.contains("hello"));
}

#[test]
fn session_resume_reset_repaints_loaded_transcript_without_duplicate_append() {
    let mut app = test_app();
    app.messages
        .push(Message::text(Role::User, "summarize this project"));
    app.message_seq_ids.push(Some(10));
    app.messages
        .push(assistant_message("Belltower is an agent harness."));
    app.message_seq_ids.push(Some(11));

    app.rebuild_committed_history_for_scrollback_reset();

    let reset = lines_to_text(&app.compose_scrollback_reset_lines());
    assert!(reset.contains("summarize this project"));
    assert!(reset.contains("Belltower is an agent harness."));
    assert_eq!(reset.matches("summarize this project").count(), 1);
    assert!(
        app.take_new_history_lines().is_none(),
        "resume reset paints committed history directly; it must not also append it"
    );
}

#[test]
fn terminal_resize_reflow_schedules_hard_clear_and_source_backed_repaint() {
    let mut app = test_app();
    app.apply_stream_event(event(
        1,
        EventPayload::MessageAppended {
            message: Message::text(Role::User, "hello"),
        },
    ));
    assert!(
        app.take_new_history_lines().is_some(),
        "the original append should be pending before resize"
    );

    app.prepare_scrollback_reflow_for_resize(40);

    assert!(app.pending_hard_clear);
    assert!(!app.pending_visible_clear);
    assert_eq!(app.transcript_output_width, 40);
    assert_eq!(
        app.input_view_width(),
        composer_wrap_width(40).saturating_sub(2)
    );
    assert_eq!(app.bottom_panel_view_width(), 40);
    let reset = lines_to_text(
        app.pending_scrollback_reset_lines
            .as_ref()
            .expect("resize should schedule source-backed repaint"),
    );
    assert!(reset.contains("Welcome to Belltower"));
    assert!(reset.contains("hello"));
    assert!(
        app.take_new_history_lines().is_none(),
        "resize repaint must not also append stale old-width history"
    );
}

#[test]
fn compose_scrollback_reset_lines_does_not_inject_banner_to_history_gap() {
    let mut app = test_app();
    app.queue_plain_history_text(
        "! /use chatgpt gpt-5.4 [ok]\n  Using chatgpt (gpt-5.4)",
        TranscriptEntryKind::Operator,
        None,
    );

    let lines = app
        .compose_scrollback_reset_lines()
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let banner_end = lines
        .iter()
        .position(|line| line == "* Type /help for a list of commands")
        .expect("banner help line");
    let command_start = lines
        .iter()
        .position(|line| line.starts_with("! /use chatgpt"))
        .expect("operator command line");

    assert_eq!(
        command_start,
        banner_end + 1,
        "startup reset should not add synthetic blank rows before first history entry"
    );
}

#[test]
fn inline_viewport_height_grows_when_active_cell_is_present() {
    let mut app = test_app();
    let idle_height = desired_inline_viewport_height(&mut app, 80, 30);
    app.apply_stream_event(event(
        1,
        EventPayload::ToolCallRequested {
            call_id: ToolCallId::new("call_list"),
            tool_name: "list".to_owned(),
            arguments: json!({ "path": "." }),
        },
    ));
    let live_height = desired_inline_viewport_height(&mut app, 80, 30);
    assert!(live_height > idle_height);
}

#[test]
fn footer_renders_session_branch_connection_model_and_cwd() {
    let app = test_app();
    let footer = lines_to_text(&render_footer_lines(&app));
    assert!(footer.contains("Session:"));
    assert!(footer.contains("local"));
    assert!(footer.contains("belltower"));
}

#[test]
fn bottom_surface_defaults_to_footer_when_no_transient_panel_is_active() {
    let app = test_app();
    let surface = render_bottom_panel(&app);
    let text = lines_to_text(&surface.lines);

    assert_eq!(surface.title, "Footer");
    assert_eq!(surface.kind, BottomSurfaceKind::Footer);
    assert!(text.contains("Session:"));
    assert!(text.contains("/tmp/belltower"));
}

#[test]
fn footer_hides_when_composer_has_draft_text() {
    let mut app = test_app();
    app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));

    let surface = render_bottom_panel(&app);

    assert_eq!(surface.kind, BottomSurfaceKind::Footer);
    assert!(surface.lines.is_empty());
}

#[test]
fn composer_body_height_tracks_input_without_extra_headroom() {
    let mut app = test_app();

    assert_eq!(composer_body_height(&app), 3);

    app.handle_paste("first\nsecond");
    assert_eq!(composer_body_height(&app), 4);

    app.handle_paste("\nthird");
    assert_eq!(composer_body_height(&app), 5);
}

#[test]
fn composer_wrap_width_reserves_prompt_gutter_and_right_padding() {
    assert_eq!(composer_wrap_width(10), 7);
    assert_eq!(composer_wrap_width(3), 1);
}

#[test]
fn composer_text_area_rect_leaves_top_and_bottom_padding() {
    let rect = composer_text_area_rect(ratatui::layout::Rect::new(4, 7, 20, 5));
    assert_eq!(rect.x, 4);
    assert_eq!(rect.y, 8);
    assert_eq!(rect.width, 19);
    assert_eq!(rect.height, 3);
}

#[test]
fn shell_uses_composer_padding_instead_of_an_extra_external_gap() {
    assert_eq!(shell_top_gap_height(), 0);
}

#[test]
fn committed_history_line_separator_does_not_insert_blank_row() {
    let mut app = test_app();

    app.queue_plain_history_text(
        "first",
        TranscriptEntryKind::Operator,
        Some(ScrollbackSeparator::None),
    );
    app.queue_plain_history_text(
        "second",
        TranscriptEntryKind::ToolCall,
        Some(ScrollbackSeparator::Line),
    );

    let text = lines_to_text(&app.take_new_history_lines().expect("history lines"));
    assert_eq!(text, "first\nsecond");
}

#[test]
fn committed_history_paragraph_separator_inserts_single_blank_row() {
    let mut app = test_app();

    app.queue_plain_history_text(
        "first",
        TranscriptEntryKind::Operator,
        Some(ScrollbackSeparator::None),
    );
    app.queue_plain_history_text(
        "second",
        TranscriptEntryKind::Assistant,
        Some(ScrollbackSeparator::Paragraph),
    );

    let text = lines_to_text(&app.take_new_history_lines().expect("history lines"));
    assert_eq!(text, "first\n\nsecond");
}

#[test]
fn committed_user_history_uses_composer_background() {
    let mut app = test_app();

    app.queue_scrollback_message(
        &Message::text(Role::User, "summarize this project"),
        Some(1),
    );

    let lines = app.take_new_history_lines().expect("history lines");
    assert_eq!(lines.len(), 3);
    assert!(lines[0].to_string().trim().is_empty());
    assert_eq!(lines[0].spans[0].style.bg, Some(COMPOSER_BACKGROUND));
    assert!(lines[1].to_string().starts_with("› summarize this project"));
    assert_eq!(lines[1].spans[0].style.bg, Some(COMPOSER_BACKGROUND));
    assert!(
        lines[1].spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
    assert!(lines[1].spans[0].style.add_modifier.contains(Modifier::DIM));
    assert!(lines[2].to_string().trim().is_empty());
    assert_eq!(lines[2].spans[0].style.bg, Some(COMPOSER_BACKGROUND));
}

#[test]
fn committed_user_history_uses_cell_padding_without_extra_paragraph_gap() {
    let mut app = test_app();
    app.queue_plain_history_text(
        "Hello! How can I help?",
        TranscriptEntryKind::Assistant,
        Some(ScrollbackSeparator::None),
    );
    app.queue_plain_history_text("summarize this project", TranscriptEntryKind::User, None);

    let lines = app.take_new_history_lines().expect("history lines");
    let text = lines_to_text(&lines);
    assert!(
        !text.contains("Hello! How can I help?\n\n"),
        "the user cell top padding should own the visual gap after assistant text"
    );
    let prompt_index = lines
        .iter()
        .position(|line| line.to_string().contains("summarize this project"))
        .expect("prompt row");
    assert!(prompt_index > 0);
    assert!(
        lines[prompt_index - 1]
            .spans
            .iter()
            .all(|span| span.style.bg == Some(COMPOSER_BACKGROUND))
    );
}

#[test]
fn committed_multiline_user_history_matches_composer_height() {
    let mut app = test_app();

    app.queue_scrollback_message(
        &Message::text(Role::User, "first line\nsecond line"),
        Some(1),
    );

    let lines = app.take_new_history_lines().expect("history lines");
    assert_eq!(lines.len(), 4);
    assert!(lines[0].to_string().trim().is_empty());
    assert!(lines[1].to_string().starts_with("› first line"));
    assert!(lines[2].to_string().starts_with("  second line"));
    assert!(lines[3].to_string().trim().is_empty());
}

#[test]
fn assistant_render_collapses_excess_blank_lines_to_single_paragraph_gap() {
    let rendered = render_message_with_options(
        &Message::text(Role::Assistant, "first\n\n\n\nsecond".to_owned()),
        false,
        TranscriptDensity::Verbose,
        80,
    );

    assert_eq!(rendered, "first\n\nsecond");
}

#[test]
fn completed_tool_block_renders_call_header_and_result() {
    let rendered = render_tool_block_entry(
        "Called",
        "list",
        "call_list",
        Some("2 entries"),
        Some(&ToolResultEnvelope {
            call_id: ToolCallId::new("call_list"),
            tool_name: "list".to_owned(),
            output: json!({ "entries": ["a", "b"] }),
            is_error: false,
            duration_ms: None,
        }),
        80,
    );

    assert!(rendered.contains("• Called list 2 entries · call_list"));
    assert!(rendered.contains("└ list: 2 entries"));
}

#[test]
fn slash_surface_collapses_when_slash_is_deleted() {
    let mut app = test_app();

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    assert_eq!(
        render_bottom_panel(&app).kind,
        BottomSurfaceKind::CommandMenu
    );

    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(render_bottom_panel(&app).kind, BottomSurfaceKind::Footer);
}

#[test]
fn spawn_command_args_support_connection_and_model_selection() {
    let args = parse_spawn_command_args(&[
        "--connection",
        "chatgpt",
        "--model",
        "gpt-5.4-mini",
        "prove",
        "the",
        "lemma",
    ])
    .expect("spawn args");

    assert_eq!(args.objective, "prove the lemma");
    assert_eq!(args.connection_id, Some(ConnectionId::new("chatgpt")));
    assert_eq!(args.model_id.as_deref(), Some("gpt-5.4-mini"));
}

#[test]
fn spawn_command_args_keep_existing_objective_only_form() {
    let args = parse_spawn_command_args(&["inspect", "the", "proof"]).expect("spawn args");

    assert_eq!(args.objective, "inspect the proof");
    assert_eq!(args.connection_id, None);
    assert_eq!(args.model_id, None);
}

#[test]
fn spawn_command_args_reject_missing_model_value() {
    let message = parse_spawn_command_args(&["--model"]).expect_err("missing value should fail");

    assert!(message.contains("Usage: /spawn"));
}

#[tokio::test]
async fn export_without_path_prefills_editable_path_prompt() {
    let mut app = test_app();

    app.handle_command("/export").await.expect("export prompt");

    assert_eq!(app.composer.input, "/export jsonl ");
    assert_eq!(app.composer.cursor, app.composer.input.len());
    assert!(
        app.active_notice()
            .is_some_and(|notice| notice.contains("export path"))
    );
}

#[test]
fn slash_surface_takes_precedence_over_pending_question_prompt() {
    let mut app = test_app();
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::Working,
        cancel_requested: false,
        pending_steer_count: 0,
        queued_messages: Vec::new(),
        pending_approvals: Vec::new(),
        pending_inputs: vec![PendingInputInspection {
            call_id: ToolCallId::new("call_ask"),
            prompt: "Pick a mode".to_owned(),
            choices: vec!["fast".to_owned(), "safe".to_owned()],
            branch_id: app.branch_id,
            turn_id: bt_core::TurnId::new(),
            settings_revision_id: default_settings_revision_id(),
            requested_at: OffsetDateTime::UNIX_EPOCH,
        }],
    });

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));

    let surface = render_bottom_panel(&app);
    assert_eq!(surface.kind, BottomSurfaceKind::CommandMenu);
    assert_eq!(surface.title, "Commands");
}

#[test]
fn pasted_text_is_inserted_as_multiline_composer_input() {
    let mut app = test_app();

    app.handle_paste("first line\r\nsecond line\rthird line");

    assert_eq!(app.composer.input, "first line\nsecond line\nthird line");
}

#[test]
fn approval_menu_uses_canonical_queue_inspection() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_shell");
    app.upsert_pending_tool(
        call_id.clone(),
        "shell".to_owned(),
        Some("cargo test".to_owned()),
    );
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::WaitingOnApproval,
        cancel_requested: false,
        pending_steer_count: 0,
        queued_messages: Vec::new(),
        pending_approvals: vec![PendingApprovalInspection {
            call_id: call_id.clone(),
            tool_name: "shell".to_owned(),
            branch_id: app.branch_id,
            turn_id: bt_core::TurnId::new(),
            settings_revision_id: default_settings_revision_id(),
            requested_at: OffsetDateTime::UNIX_EPOCH,
            request_snapshot: None,
        }],
        pending_inputs: Vec::new(),
    });

    assert!(app.approval_menu_is_active());
    let panel = render_bottom_panel(&app);
    let text = lines_to_text(&panel.lines);
    assert!(text.contains("approval: shell"));
    assert!(text.contains("cargo test"));
}

#[test]
fn approval_menu_surfaces_over_draft_input_and_slash_menu() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_shell");
    app.upsert_pending_tool(
        call_id.clone(),
        "shell".to_owned(),
        Some("cargo test".to_owned()),
    );
    app.composer.input = "/status with draft".to_owned();
    app.composer.cursor = app.composer.input.len();
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::WaitingOnApproval,
        cancel_requested: false,
        pending_steer_count: 0,
        queued_messages: Vec::new(),
        pending_approvals: vec![PendingApprovalInspection {
            call_id: call_id.clone(),
            tool_name: "shell".to_owned(),
            branch_id: app.branch_id,
            turn_id: bt_core::TurnId::new(),
            settings_revision_id: default_settings_revision_id(),
            requested_at: OffsetDateTime::UNIX_EPOCH,
            request_snapshot: None,
        }],
        pending_inputs: Vec::new(),
    });

    let panel = render_bottom_panel(&app);
    assert_eq!(panel.kind, BottomSurfaceKind::Approval);
    assert!(lines_to_text(&panel.lines).contains("approval: shell"));

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.bottom_shell.approval_menu_selection, 1);
    assert_eq!(
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ChatAction::ApproveLastPendingSession)
    );
    assert_eq!(app.composer.input, "/status with draft");
}

#[tokio::test]
async fn approval_menu_resolves_while_send_is_in_flight() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_shell");
    app.upsert_pending_tool(
        call_id.clone(),
        "shell".to_owned(),
        Some("cargo test".to_owned()),
    );
    app.composer.input = "draft prompt".to_owned();
    app.composer.cursor = app.composer.input.len();
    app.pending_send = Some(tokio::spawn(async {
        std::future::pending::<std::result::Result<PendingSendCompletion, String>>().await
    }));
    app.pending_send_started_at = Some(Instant::now());
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::WaitingOnApproval,
        cancel_requested: false,
        pending_steer_count: 0,
        queued_messages: Vec::new(),
        pending_approvals: vec![PendingApprovalInspection {
            call_id: call_id.clone(),
            tool_name: "shell".to_owned(),
            branch_id: app.branch_id,
            turn_id: bt_core::TurnId::new(),
            settings_revision_id: default_settings_revision_id(),
            requested_at: OffsetDateTime::UNIX_EPOCH,
            request_snapshot: None,
        }],
        pending_inputs: Vec::new(),
    });

    assert!(app.has_pending_request());
    assert!(app.approval_menu_is_active());
    assert_eq!(render_bottom_panel(&app).kind, BottomSurfaceKind::Approval);

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.bottom_shell.approval_menu_selection, 1);

    app.start_resolve_last_pending(true, bt_core::ApprovalScope::Once, None)
        .await
        .expect("approval should dispatch while a send is in flight");
    assert!(app.pending_approval.is_some());

    if let Some(handle) = app.pending_approval.take() {
        handle.abort();
    }
    if let Some(handle) = app.pending_send.take() {
        handle.abort();
    }
}

#[test]
fn question_panel_uses_canonical_queue_inspection() {
    let mut app = test_app();
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::Working,
        cancel_requested: false,
        pending_steer_count: 0,
        queued_messages: Vec::new(),
        pending_approvals: Vec::new(),
        pending_inputs: vec![PendingInputInspection {
            call_id: ToolCallId::new("call_ask"),
            prompt: "Pick a mode".to_owned(),
            choices: vec!["fast".to_owned(), "safe".to_owned()],
            branch_id: app.branch_id,
            turn_id: bt_core::TurnId::new(),
            settings_revision_id: default_settings_revision_id(),
            requested_at: OffsetDateTime::UNIX_EPOCH,
        }],
    });

    assert!(app.question_panel_is_active());
    let panel = render_bottom_panel(&app);
    let text = lines_to_text(&panel.lines);
    assert_eq!(panel.kind, BottomSurfaceKind::Question);
    assert!(text.contains("Question 1/1"));
    assert!(text.contains("Pick a mode"));
    assert!(text.contains("› 1.  fast"));
    assert!(text.contains("2.  safe"));
    assert!(text.contains("enter to submit answer"));
}

#[test]
fn question_panel_stays_active_while_answer_is_typed() {
    let mut app = test_app();
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::WaitingOnInput,
        cancel_requested: false,
        pending_steer_count: 0,
        queued_messages: Vec::new(),
        pending_approvals: Vec::new(),
        pending_inputs: vec![PendingInputInspection {
            call_id: ToolCallId::new("call_ask"),
            prompt: "Where is the calendar?".to_owned(),
            choices: Vec::new(),
            branch_id: app.branch_id,
            turn_id: bt_core::TurnId::new(),
            settings_revision_id: default_settings_revision_id(),
            requested_at: OffsetDateTime::UNIX_EPOCH,
        }],
    });

    for ch in "default location".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }

    let panel = render_bottom_panel(&app);
    let text = lines_to_text(&panel.lines);
    assert_eq!(panel.kind, BottomSurfaceKind::Question);
    assert!(text.contains("Where is the calendar?"));
    assert!(text.contains("› default location"));
}

#[test]
fn question_panel_owns_bottom_surface_without_duplicate_waiting_status() {
    let mut app = test_app();
    app.set_task_status("Waiting on input", Some("ask call_ask".to_owned()));
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::WaitingOnInput,
        cancel_requested: false,
        pending_steer_count: 0,
        queued_messages: Vec::new(),
        pending_approvals: Vec::new(),
        pending_inputs: vec![PendingInputInspection {
            call_id: ToolCallId::new("call_ask"),
            prompt: "Where is the calendar?".to_owned(),
            choices: Vec::new(),
            branch_id: app.branch_id,
            turn_id: bt_core::TurnId::new(),
            settings_revision_id: default_settings_revision_id(),
            requested_at: OffsetDateTime::UNIX_EPOCH,
        }],
    });

    assert!(shell_aux_lines(&app, 80).is_empty());
    assert_eq!(render_bottom_panel(&app).kind, BottomSurfaceKind::Question);
}

#[test]
fn question_choice_selection_uses_arrow_keys() {
    let mut app = test_app();
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::WaitingOnInput,
        cancel_requested: false,
        pending_steer_count: 0,
        queued_messages: Vec::new(),
        pending_approvals: Vec::new(),
        pending_inputs: vec![PendingInputInspection {
            call_id: ToolCallId::new("call_ask"),
            prompt: "Pick a mode".to_owned(),
            choices: vec!["fast".to_owned(), "safe".to_owned()],
            branch_id: app.branch_id,
            turn_id: bt_core::TurnId::new(),
            settings_revision_id: default_settings_revision_id(),
            requested_at: OffsetDateTime::UNIX_EPOCH,
        }],
    });

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

    let panel = render_bottom_panel(&app);
    let text = lines_to_text(&panel.lines);
    assert!(text.contains("  1.  fast"));
    assert!(text.contains("› 2.  safe"));
}

#[test]
fn question_panel_wraps_long_prompts_inside_bottom_surface() {
    let mut app = test_app();
    app.set_bottom_panel_view_width(24);
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::Working,
        cancel_requested: false,
        pending_steer_count: 0,
        queued_messages: Vec::new(),
        pending_approvals: Vec::new(),
        pending_inputs: vec![PendingInputInspection {
            call_id: ToolCallId::new("call_ask"),
            prompt: "Tell me which calendar file I should inspect and include the full path."
                .to_owned(),
            choices: Vec::new(),
            branch_id: app.branch_id,
            turn_id: bt_core::TurnId::new(),
            settings_revision_id: default_settings_revision_id(),
            requested_at: OffsetDateTime::UNIX_EPOCH,
        }],
    });

    let panel = render_bottom_panel(&app);
    let text = lines_to_text(&panel.lines);

    assert_eq!(panel.kind, BottomSurfaceKind::Question);
    assert!(panel.lines.len() > 3);
    assert!(text.contains("Tell me which"));
    assert!(text.contains("Type your answer"));
    assert!(text.contains("enter to submit"));
}

#[test]
fn ask_tool_request_does_not_render_live_tool_cell() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_ask");

    app.apply_stream_event(event(
        1,
        EventPayload::ToolCallRequested {
            call_id,
            tool_name: "ask".to_owned(),
            arguments: json!({ "question": "Pick a mode" }),
        },
    ));

    let live = lines_to_text(&app.active_view_lines(80));
    assert!(!live.contains("Calling ask"));
    assert!(!live.contains("Called ask"));
    let task_status = app.task_status.as_ref().expect("task status");
    assert_eq!(task_status.header, "Waiting on input");
    assert_eq!(task_status.detail.as_deref(), Some("ask call_ask"));
    assert!(app.active_notice().is_none());
}

#[test]
fn canonical_ask_message_stays_transient_until_answered() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_ask");

    app.apply_stream_event(event(
        1,
        EventPayload::ToolCallRequested {
            call_id: call_id.clone(),
            tool_name: "ask".to_owned(),
            arguments: json!({ "question": "Pick a mode" }),
        },
    ));
    app.apply_stream_event(event(
        2,
        EventPayload::MessageAppended {
            message: assistant_tool_call_message(
                "ask",
                &call_id.to_string(),
                json!({ "question": "Pick a mode" }),
            ),
        },
    ));

    assert!(app.take_new_history_lines().is_none());
}

#[test]
fn ask_tool_result_does_not_commit_tool_block() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_ask");

    app.apply_stream_event(event(
        1,
        EventPayload::ToolCallRequested {
            call_id: call_id.clone(),
            tool_name: "ask".to_owned(),
            arguments: json!({ "question": "Pick a mode" }),
        },
    ));
    app.apply_stream_event(event(
        2,
        EventPayload::ToolExecutionFinished {
            call_id: call_id.clone(),
            tool_name: "ask".to_owned(),
            result: ToolResultEnvelope {
                call_id,
                tool_name: "ask".to_owned(),
                is_error: false,
                output: json!({ "response": "safe" }),
                duration_ms: Some(5),
            },
        },
    ));

    assert!(app.take_new_history_lines().is_none());
    let live = lines_to_text(&app.active_view_lines(80));
    assert!(!live.contains("Calling ask"));
    assert!(!live.contains("Called ask"));
}

#[test]
fn canonical_ask_result_commits_completed_history_cell() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_ask");

    app.apply_stream_event(event(
        1,
        EventPayload::MessageAppended {
            message: assistant_tool_call_message(
                "ask",
                &call_id.to_string(),
                json!({ "question": "Pick a mode" }),
            ),
        },
    ));

    app.apply_stream_event(event(
        2,
        EventPayload::MessageAppended {
            message: assistant_tool_result_message(
                "ask",
                &call_id.to_string(),
                json!({ "response": "safe" }),
            ),
        },
    ));

    let committed = lines_to_text(
        &app.take_new_history_lines()
            .expect("canonical ask answer should commit to history"),
    );
    assert!(committed.contains("Questions 1/1 answered"));
    assert!(committed.contains("Pick a mode"));
    assert!(committed.contains("answer: safe"));
    assert!(!committed.contains("Question:"));
    assert!(!committed.contains("Answer:"));
    assert!(!committed.contains("Calling ask"));
    assert!(!committed.contains("Called ask"));
}

#[test]
fn tool_role_ask_result_commits_completed_history_cell() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_ask");

    app.apply_stream_event(event(
        1,
        EventPayload::MessageAppended {
            message: assistant_tool_call_message(
                "ask",
                &call_id.to_string(),
                json!({ "question": "Pick a mode" }),
            ),
        },
    ));

    app.apply_stream_event(event(
        2,
        EventPayload::MessageAppended {
            message: tool_result_message("ask", &call_id.to_string(), json!({ "response": "Yes" })),
        },
    ));

    let committed = lines_to_text(
        &app.take_new_history_lines()
            .expect("tool-role ask answer should commit to history"),
    );
    assert!(committed.contains("Questions 1/1 answered"));
    assert!(committed.contains("Pick a mode"));
    assert!(committed.contains("answer: Yes"));
    assert!(!committed.contains("└ Yes"));
    assert!(!committed.contains("Question:"));
    assert!(!committed.contains("Answer:"));
    assert!(!committed.contains("Calling ask"));
    assert!(!committed.contains("Called ask"));
}

#[test]
fn footer_shows_canonical_queue_summary() {
    let mut app = test_app();
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::Working,
        cancel_requested: true,
        pending_steer_count: 1,
        queued_messages: vec![bt_core::QueuedMessageInspection {
            branch_id: app.branch_id,
            enqueued_at: OffsetDateTime::UNIX_EPOCH,
            message: Message::text(Role::User, "queued follow-up"),
            settings_revision_id: default_settings_revision_id(),
        }],
        pending_approvals: Vec::new(),
        pending_inputs: Vec::new(),
    });

    let footer = lines_to_text(&render_footer_lines(&app));
    assert!(footer.contains("queue messages=1"));
    assert!(footer.contains("cancel_requested=true"));
}

#[test]
fn shell_aux_lines_show_working_status_and_queued_follow_ups() {
    let mut app = test_app();
    app.active_turn.live = true;
    app.active_turn.started_at = Some(Instant::now());
    app.set_task_status("Working", Some("openai (gpt-5.4-mini)".to_owned()));
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::Working,
        cancel_requested: false,
        pending_steer_count: 0,
        queued_messages: vec![bt_core::QueuedMessageInspection {
            branch_id: app.branch_id,
            enqueued_at: OffsetDateTime::UNIX_EPOCH,
            message: Message::text(Role::User, "summarize this project"),
            settings_revision_id: default_settings_revision_id(),
        }],
        pending_approvals: Vec::new(),
        pending_inputs: Vec::new(),
    });

    let text = lines_to_text(&shell_aux_lines(&app, 80));

    assert!(text.contains("• Working"));
    assert!(text.contains("└ openai (gpt-5.4-mini)"));
    assert!(text.contains("• Queued follow-up messages"));
    assert!(text.contains("summarize this project"));
}

#[test]
fn shell_aux_lines_remain_visible_while_slash_menu_is_open() {
    let mut app = test_app();
    app.active_turn.live = true;
    app.active_turn.started_at = Some(Instant::now());
    app.set_task_status("Working", Some("openai (gpt-5.4-mini)".to_owned()));
    app.composer.input = "/".to_owned();
    app.composer.cursor = app.composer.input.len();

    assert!(app.should_show_command_menu());
    let text = lines_to_text(&shell_aux_lines(&app, 80));

    assert!(text.contains("• Working"));
    assert!(text.contains("└ openai (gpt-5.4-mini)"));
}

#[test]
fn shell_aux_lines_prefer_task_status_over_generic_status_string() {
    let mut app = test_app();
    app.active_turn.live = true;
    app.active_turn.started_at = Some(Instant::now());
    app.status = "Ready. 3 messages loaded.".to_owned();
    app.set_task_status("Waiting on approval", Some("shell call_123".to_owned()));

    let text = lines_to_text(&shell_aux_lines(&app, 80));

    assert!(text.contains("• Waiting on approval"));
    assert!(text.contains("└ shell call_123"));
    assert!(!text.contains("Ready. 3 messages loaded."));
}

#[test]
fn shell_aux_lines_fall_back_to_runtime_state_without_task_status() {
    let mut app = test_app();
    app.active_turn.live = true;
    app.active_turn.started_at = Some(Instant::now());
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::WaitingOnApproval,
        cancel_requested: false,
        pending_steer_count: 0,
        queued_messages: Vec::new(),
        pending_approvals: Vec::new(),
        pending_inputs: Vec::new(),
    });

    let text = lines_to_text(&shell_aux_lines(&app, 80));

    assert!(text.contains("• Waiting on approval"));
}

#[test]
fn completion_requested_updates_task_status_without_overwriting_stable_status() {
    let mut app = test_app();
    app.status = "Ready. 3 messages loaded.".to_owned();

    app.apply_stream_event(event(
        1,
        EventPayload::CompletionRequested {
            llm_call_ordinal: 0,
            provider: "openai".to_owned(),
            model: "gpt-5.4-mini".to_owned(),
            message_count: 1,
        },
    ));

    let task_status = app.task_status.as_ref().expect("task status");
    assert_eq!(task_status.header, "Working");
    assert_eq!(task_status.detail.as_deref(), Some("openai (gpt-5.4-mini)"));
    assert_eq!(app.status, "Ready. 3 messages loaded.");
}

#[test]
fn tool_approval_requested_updates_task_status_without_overwriting_stable_status() {
    let mut app = test_app();
    app.status = "Ready. 3 messages loaded.".to_owned();

    app.apply_stream_event(event(
        1,
        EventPayload::ToolApprovalRequested {
            tool_name: "shell".to_owned(),
            call_id: ToolCallId::new("call_123"),
            snapshot: None,
        },
    ));

    let task_status = app.task_status.as_ref().expect("task status");
    assert_eq!(task_status.header, "Waiting on approval");
    assert_eq!(task_status.detail.as_deref(), Some("shell call_123"));
    assert_eq!(app.status, "Ready. 3 messages loaded.");
}

#[test]
fn rebuilt_history_collapses_ask_exchange_into_completed_entry() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_ask");

    app.messages.push(assistant_tool_call_message(
        "ask",
        &call_id.to_string(),
        json!({ "question": "Pick a mode" }),
    ));
    app.message_seq_ids.push(Some(1));
    app.messages.push(tool_result_message(
        "ask",
        &call_id.to_string(),
        json!({ "response": "safe" }),
    ));
    app.message_seq_ids.push(Some(2));

    app.rebuild_committed_history_from_transcript();

    let committed = lines_to_text(
        &app.take_new_history_lines()
            .expect("rebuilt ask exchange should render in history"),
    );
    assert!(committed.contains("Questions 1/1 answered"));
    assert!(committed.contains("Pick a mode"));
    assert!(committed.contains("answer: safe"));
    assert!(!committed.contains("Question:"));
    assert!(!committed.contains("Answer:"));
}

#[test]
fn active_view_lines_exclude_shell_status_and_queue_preview() {
    let mut app = test_app();
    app.pending_send_started_at = Some(Instant::now());
    app.active_turn.live = true;
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::Working,
        cancel_requested: false,
        pending_steer_count: 0,
        queued_messages: vec![bt_core::QueuedMessageInspection {
            branch_id: app.branch_id,
            enqueued_at: OffsetDateTime::UNIX_EPOCH,
            message: Message::text(Role::User, "summarize this project"),
            settings_revision_id: default_settings_revision_id(),
        }],
        pending_approvals: Vec::new(),
        pending_inputs: Vec::new(),
    });

    let text = lines_to_text(&app.active_view_lines(80));

    assert!(!text.contains("• Working"));
    assert!(!text.contains("• Queued follow-up messages"));
    assert!(!text.contains("summarize this project"));
}

#[test]
fn turn_busy_uses_canonical_runtime_state() {
    let mut app = test_app();
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::Working,
        cancel_requested: false,
        pending_steer_count: 0,
        queued_messages: Vec::new(),
        pending_approvals: Vec::new(),
        pending_inputs: Vec::new(),
    });

    assert!(app.turn_is_busy());
    assert!(app.active_turn_is_live());
}

#[test]
fn ctrl_c_cancels_when_runtime_reports_active_turn() {
    let mut app = test_app();
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::Working,
        cancel_requested: false,
        pending_steer_count: 0,
        queued_messages: Vec::new(),
        pending_approvals: Vec::new(),
        pending_inputs: Vec::new(),
    });

    let action = app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(action, Some(ChatAction::Cancel));
    assert!(!app.should_quit);
}

#[test]
fn ctrl_j_inserts_newline_via_composer_binding_table() {
    let mut app = test_app();

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));

    assert_eq!(app.composer.input, "a\nb");
}

#[test]
fn arrow_keys_move_within_multiline_composer_before_history_browse() {
    let mut app = test_app();
    app.handle_paste("one\ntwo\nsix");
    assert_eq!(app.composer.cursor, "one\ntwo\nsix".len());

    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.composer.cursor, "one\ntwo".len());
    assert_eq!(app.composer.history_browse_index, None);

    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.composer.cursor, "one".len());
    assert_eq!(app.composer.history_browse_index, None);

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.composer.cursor, "one\ntwo".len());
    assert_eq!(app.composer.history_browse_index, None);
}

#[test]
fn arrow_keys_move_across_wrapped_composer_rows_before_history_browse() {
    let mut app = test_app();
    app.set_input_view_width(6);
    app.handle_paste("abcdefghi");
    app.composer.cursor = 3;

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.composer.cursor, 7);
    assert_eq!(app.composer.history_browse_index, None);

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.composer.cursor, "abcdefghi".len());
    assert_eq!(app.composer.history_browse_index, None);

    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.composer.cursor, 7);
    assert_eq!(app.composer.history_browse_index, None);
}

#[test]
fn ctrl_k_kills_text_after_cursor() {
    let mut app = test_app();
    app.handle_paste("alpha beta gamma");
    app.move_input_cursor_start();
    app.move_input_cursor_word_right();
    app.move_input_cursor_right();

    app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));

    assert_eq!(app.composer.input, "alpha ");
    app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(app.composer.input, "alpha beta gamma");
}

#[test]
fn ctrl_u_kills_to_line_start_and_yanks_it_back() {
    let mut app = test_app();
    app.handle_paste("alpha beta gamma");
    app.move_input_cursor_end();

    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));

    assert!(app.composer.input.is_empty());
    app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(app.composer.input, "alpha beta gamma");
}

#[test]
fn ctrl_w_kills_previous_word_and_yanks_it_back() {
    let mut app = test_app();
    app.handle_paste("alpha beta gamma");
    app.move_input_cursor_end();

    app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));

    assert_eq!(app.composer.input, "alpha beta ");
    app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(app.composer.input, "alpha beta gamma");
}

#[test]
fn kill_buffer_survives_composer_clear() {
    let mut app = test_app();
    app.handle_paste("alpha beta");
    app.move_input_cursor_end();
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));

    // Models the composer reset path used after slash-command dispatch.
    app.composer.input.clear();
    app.composer.cursor = 0;
    app.clear_input_history_browse();

    app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(app.composer.input, "alpha beta");
}

#[test]
fn command_menu_scroll_advances_without_recentering() {
    let mut app = test_app();
    app.handle_paste("/");
    let visible_rows = crate::bottom_pane::command_menu_visible_row_count();
    let items_len = app.current_command_menu_items().len();
    assert!(items_len > visible_rows + 1);
    assert_eq!(app.bottom_shell.command_menu_selection, 0);
    assert_eq!(app.bottom_shell.command_menu_scroll_top, 0);

    for expected_selection in 1..=visible_rows {
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.bottom_shell.command_menu_selection, expected_selection);
    }

    assert_eq!(app.bottom_shell.command_menu_scroll_top, 1);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.bottom_shell.command_menu_selection, visible_rows + 1);
    assert_eq!(app.bottom_shell.command_menu_scroll_top, 2);
}

#[test]
fn alt_left_and_right_move_by_word() {
    let mut app = test_app();
    app.handle_paste("alpha beta gamma");
    app.move_input_cursor_end();

    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
    assert_eq!(app.composer.cursor, "alpha beta ".len());

    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
    assert_eq!(app.composer.cursor, "alpha ".len());

    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
    assert_eq!(app.composer.cursor, "alpha beta".len());
}

#[test]
fn option_delete_escape_variants_delete_previous_word() {
    for code in [
        KeyCode::Backspace,
        KeyCode::Char('\u{7f}'),
        KeyCode::Char('\u{8}'),
    ] {
        let mut app = test_app();
        app.handle_paste("alpha beta gamma");
        app.move_input_cursor_end();

        app.handle_key(KeyEvent::new(code, KeyModifiers::ALT));

        assert_eq!(app.composer.input, "alpha beta ");
        assert_eq!(app.composer.cursor, "alpha beta ".len());
    }
}

#[test]
fn option_backspace_meta_variants_delete_previous_word() {
    for modifiers in [
        KeyModifiers::META,
        KeyModifiers::SUPER,
        KeyModifiers::HYPER,
        KeyModifiers::ALT | KeyModifiers::META,
    ] {
        let mut app = test_app();
        app.handle_paste("alpha beta gamma");
        app.move_input_cursor_end();

        app.handle_key(KeyEvent::new(KeyCode::Backspace, modifiers));

        assert_eq!(app.composer.input, "alpha beta ");
        assert_eq!(app.composer.cursor, "alpha beta ".len());
    }
}

#[test]
fn option_del_escape_meta_variant_deletes_previous_word() {
    let mut app = test_app();
    app.handle_paste("alpha beta gamma");
    app.move_input_cursor_end();

    app.handle_key(KeyEvent::new(KeyCode::Char('\u{7f}'), KeyModifiers::META));

    assert_eq!(app.composer.input, "alpha beta ");
    assert_eq!(app.composer.cursor, "alpha beta ".len());
}

#[test]
fn escape_prefixed_option_backspace_deletes_previous_word() {
    for code in [
        KeyCode::Backspace,
        KeyCode::Char('\u{7f}'),
        KeyCode::Char('\u{8}'),
    ] {
        let mut app = test_app();
        app.handle_paste("alpha beta gamma");
        app.move_input_cursor_end();

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.should_quit);
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));

        assert_eq!(app.composer.input, "alpha beta ");
        assert_eq!(app.composer.cursor, "alpha beta ".len());
        assert!(!app.should_quit);
    }
}

#[test]
fn plain_escape_still_exits_after_prefix_timeout() {
    let mut app = test_app();

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.should_quit);
    app.flush_pending_escape_prefix_if_expired(
        Instant::now() + ESCAPE_PREFIX_TIMEOUT + Duration::from_millis(1),
    );

    assert!(app.should_quit);
}

#[test]
fn repeat_key_events_advance_command_menu_selection() {
    let mut app = test_app();
    app.handle_paste("/d");
    assert!(app.should_show_command_menu());
    let initial = app.bottom_shell.command_menu_selection;

    app.handle_key(KeyEvent {
        code: KeyCode::Down,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Repeat,
        state: KeyEventState::NONE,
    });

    assert_ne!(app.bottom_shell.command_menu_selection, initial);
}

#[tokio::test]
async fn queued_send_outcome_clears_optimistic_turn_state() {
    let mut app = test_app();
    app.mark_live_turn_start();
    app.active_turn.live = true;
    app.active_turn.optimistic_user_text = Some("hello".to_owned());
    app.last_metadata_refresh = Instant::now();

    app.apply_message_submission_outcome(
        SendMessageResponse {
            session_id: app.session_id,
            branch_id: app.branch_id,
            outcome: SendMessageOutcome::Queued { position: 2 },
        },
        None,
        true,
    )
    .await
    .expect("queued outcome should apply");

    assert!(!app.active_turn.live);
    assert!(app.active_turn.optimistic_user_text.is_none());
    assert!(app.active_notice().is_none());
}

#[tokio::test]
async fn slash_busy_gate_uses_canonical_runtime_state() {
    let mut app = test_app();
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::Working,
        cancel_requested: false,
        pending_steer_count: 0,
        queued_messages: Vec::new(),
        pending_approvals: Vec::new(),
        pending_inputs: Vec::new(),
    });
    app.composer.input = "/use openai o4-mini".to_owned();
    app.composer.cursor = app.composer.input.len();

    app.send_input().await.expect("send input");

    assert_eq!(app.composer.input, "/use openai o4-mini");
    assert_eq!(
        app.active_notice(),
        Some("Wait for the current request before running that command.")
    );
}

#[tokio::test]
async fn operator_shell_busy_gate_uses_canonical_runtime_state() {
    let mut app = test_app();
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::Working,
        cancel_requested: false,
        pending_steer_count: 0,
        queued_messages: Vec::new(),
        pending_approvals: Vec::new(),
        pending_inputs: Vec::new(),
    });
    app.composer.input = "! echo hi".to_owned();
    app.composer.cursor = app.composer.input.len();

    app.send_input().await.expect("send input");

    assert!(app.pending_shell_command.is_none());
    assert_eq!(
        app.active_notice(),
        Some(
            "Operator shell commands do not enter the session queue. Wait for the current request to finish."
        )
    );
}

#[tokio::test]
async fn pending_input_answer_does_not_replace_in_flight_send() {
    let mut app = test_app();
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::WaitingOnInput,
        cancel_requested: false,
        pending_steer_count: 0,
        queued_messages: Vec::new(),
        pending_approvals: Vec::new(),
        pending_inputs: vec![PendingInputInspection {
            call_id: ToolCallId::new("call_ask"),
            prompt: "Pick a mode".to_owned(),
            choices: Vec::new(),
            branch_id: app.branch_id,
            turn_id: bt_core::TurnId::new(),
            settings_revision_id: default_settings_revision_id(),
            requested_at: OffsetDateTime::UNIX_EPOCH,
        }],
    });
    app.pending_send = Some(tokio::spawn(async { Ok(PendingSendCompletion::Ack) }));
    let original_started_at = Some(Instant::now() - Duration::from_secs(5));
    app.pending_send_started_at = original_started_at;

    app.start_answer_pending_input(json!("safe"));

    assert_eq!(
        app.active_notice(),
        Some("Wait for the current question answer to finish.")
    );
    assert_eq!(app.pending_send_started_at, original_started_at);
}

#[tokio::test]
async fn shell_ack_clears_stale_working_status() {
    let mut app = test_app();
    app.pending_shell_command = Some("pwd".to_owned());
    app.pending_send = Some(tokio::spawn(async { Ok(PendingSendCompletion::Ack) }));
    app.pending_send_started_at = Some(Instant::now());
    app.active_turn.live = true;
    app.active_turn.started_at = Some(Instant::now());
    app.last_metadata_refresh = Instant::now();
    app.set_task_status("Running", Some("!pwd".to_owned()));
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::Working,
        cancel_requested: false,
        pending_steer_count: 0,
        queued_messages: Vec::new(),
        pending_approvals: Vec::new(),
        pending_inputs: Vec::new(),
    });
    tokio::task::yield_now().await;

    app.poll_send_completion()
        .await
        .expect("shell ACK should apply");

    assert!(app.pending_shell_command.is_none());
    assert!(app.pending_send.is_none());
    assert!(app.task_status.is_none());
    assert!(!app.active_turn.live);
    assert_eq!(
        app.queue_inspection
            .as_ref()
            .map(|state| state.runtime_state),
        Some(SessionRuntimeState::Idle)
    );
    assert!(shell_aux_lines(&app, 80).is_empty());
}

#[test]
fn execution_output_renders_canonical_turn_provenance() {
    let turn_id = bt_core::TurnId::new();
    let branch_id = BranchId::new();
    let execution = SessionExecutionInspection {
        session_id: SessionId::new(),
        total_events: 3,
        event_counts: TraceEventCounts::default(),
        related_sessions: Vec::new(),
        turns: vec![TraceTurnInspection {
            turn_id: turn_id.clone(),
            branch_id: branch_id.clone(),
            settings_revision_id: default_settings_revision_id(),
            source: Some(TurnStartSource::ApprovalResume),
            resumed_from_call_id: Some(ToolCallId::new("call-resume-1")),
            provider: "openai".to_owned(),
            model: "gpt-5.1".to_owned(),
            started_at: OffsetDateTime::UNIX_EPOCH,
            finished_at: Some(OffsetDateTime::UNIX_EPOCH),
            status: Some("completed".to_owned()),
            finish_reason: Some("stop".to_owned()),
            latency_ms: Some(10),
            event_seq_start: Some(1),
            event_seq_end: Some(3),
            event_count: 3,
            raw_chunk_count: 0,
            llm_call_count: 1,
            approval_pause_count: 1,
            resumed_after_approval: true,
            event_counts: TraceEventCounts::default(),
            usage: None,
            cost: None,
            context_manifests: vec![ContextManifest {
                turn_id,
                branch_id: branch_id.clone(),
                llm_call_ordinal: 1,
                provider: "openai".to_owned(),
                model: "gpt-5.1".to_owned(),
                settings_revision_id: default_settings_revision_id(),
                context_boundary_seq_id: Some(2),
                system_prompt: ContextSystemPromptRef {
                    present: true,
                    char_count: 96,
                },
                messages: vec![ContextMessageRef {
                    message_id: MessageId::new(),
                    role: Role::User,
                    source_branch_id: Some(branch_id),
                    source_seq_id: Some(1),
                    part_count: 1,
                    visible_text_chars: 18,
                }],
                tools: vec![ContextToolRef {
                    name: "shell".to_owned(),
                    risk_class: ToolRiskClass::Moderate,
                    is_read_only: false,
                    execution_mode: ToolExecutionMode::UserInput,
                    should_defer: false,
                }],
                attachments: Vec::new(),
                max_tokens: Some(1024),
                thinking: None,
                compacted: false,
            }],
            tool_calls: Vec::new(),
        }],
    };

    let output = render_execution_output(&execution, 5);
    assert!(output.contains("source=approval_resume"));
    assert!(output.contains("resumed_from=call-res"));
    assert!(output.contains("contexts=1"));
    assert!(output.contains("context_boundary=2"));
    assert!(output.contains("context_messages=1"));
    assert!(output.contains("context_tools=1"));
    assert_eq!(
        render_turn_start_source(&TurnStartSource::RelatedSessionMessage),
        "related_session_message"
    );
}

#[test]
fn help_output_mentions_defaults_command() {
    let output = command_help_output(false);
    assert!(
        output.contains("/defaults [connection <id>|model <model-id>|use <connection> [model]]")
    );
    assert!(!output.contains("/approve [once|session|always]"));
    assert!(!output.contains("/queue [clear]"));
}

#[test]
fn help_all_includes_hidden_slash_commands() {
    let output = command_help_output(true);
    assert!(output.contains("/resume [session-id]"));
    assert!(output.contains("/doctor"));
}

#[test]
fn root_slash_menu_prefers_primary_commands() {
    let mut app = test_app();
    app.handle_paste("/");
    let items = app.current_command_menu_items();

    assert!(items.iter().any(|item| item.label.starts_with("/help")));
    assert!(items.iter().any(|item| item.label.starts_with("/use")));
    assert!(!items.iter().any(|item| item.label.starts_with("/approve")));
    assert!(!items.iter().any(|item| item.label.starts_with("/queue")));
    assert!(!items.iter().any(|item| item.label.starts_with("/resume")));
}

#[test]
fn explicit_prefix_reveals_hidden_slash_commands() {
    let mut app = test_app();
    app.handle_paste("/res");
    let items = app.current_command_menu_items();

    assert!(items.iter().any(|item| item.label.starts_with("/resume")));
}

#[test]
fn contextual_only_commands_are_rejected_from_slash_surface() {
    let error = resolve_command("/approve").expect_err("approve should be contextual only");
    assert!(error.contains("approval chooser"));
}

#[test]
fn defaults_command_completion_suggests_actions() {
    let app = test_app();
    let completion = completion_for_input(
        "/defaults ",
        &app.connection_id,
        app.connection_model.as_deref(),
        &app.connections,
        &app.connection_models,
    )
    .expect("defaults actions should complete");
    assert!(
        completion
            .candidates
            .iter()
            .any(|candidate| candidate == "connection")
    );
    assert!(
        completion
            .candidates
            .iter()
            .any(|candidate| candidate == "model")
    );
    assert!(
        completion
            .candidates
            .iter()
            .any(|candidate| candidate == "use")
    );
}

#[test]
fn shift_enter_preserves_committed_scrollback_history() {
    let mut app = test_app();
    app.queue_scrollback_message(&Message::text(Role::User, "existing history"), Some(1));
    app.take_new_history_lines().expect("committed history");

    let before = lines_to_text(&app.compose_scrollback_reset_lines());

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    app.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));

    let after = lines_to_text(&app.compose_scrollback_reset_lines());
    assert_eq!(before, after);
    assert_eq!(app.composer.input, "\nm");
}

#[test]
fn double_shift_enter_keeps_composer_blank_lines_visible() {
    let mut app = test_app();
    app.set_input_view_width(40);

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));

    assert_eq!(app.composer.input, "\n\nhi");
    assert_eq!(
        app.input_visual_lines(),
        vec!["".to_owned(), "".to_owned(), "hi".to_owned()]
    );

    let rendered_text = render_input_text(&app);
    let rendered = lines_to_text(&rendered_text.lines);
    assert!(rendered.contains("› "));
    assert!(rendered.contains("  hi"));
}

#[test]
fn composer_space_advances_cursor_before_next_word() {
    let mut app = test_app();
    app.set_input_view_width(40);

    for ch in "hello ".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }

    assert_eq!(app.composer.input, "hello ");
    assert_eq!(app.input_cursor_position(), (6, 0));
    assert_eq!(app.input_visual_lines(), vec!["hello ".to_owned()]);

    app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));

    assert_eq!(app.composer.input, "hello w");
    assert_eq!(app.input_cursor_position(), (7, 0));
    assert_eq!(app.input_visual_lines(), vec!["hello w".to_owned()]);
}

#[test]
fn operator_surface_help_matches_fixture() {
    let actual = command_help_output(false);
    assert_operator_fixture("help.txt", &actual);
}

#[test]
fn operator_surface_status_matches_fixture() {
    let (inspection, _, _, _, _, _) = sample_operator_surface_matrix();
    let actual = render_status_output(
        &inspection,
        &ConnectionId::new("openai"),
        "gpt-5.4-mini",
        SessionToolMode::Extended,
    );

    assert_operator_fixture("status.txt", &actual);
}

#[test]
fn operator_surface_doctor_matches_fixture_and_fits_80_columns() {
    let (inspection, connections, backends, report, servers, tools) =
        sample_operator_surface_matrix();
    let actual = render_doctor_output(
        &sample_health(),
        &inspection,
        &connections,
        &backends,
        &servers,
        &tools,
        &ConnectionId::new("openai"),
        "gpt-5.4-mini",
        SessionToolMode::Extended,
    );

    assert_operator_fixture("doctor.txt", &actual);
    assert!(
        actual.lines().all(|line| crate::display_width(line) <= 80),
        "doctor output exceeded 80 columns:\n{actual}"
    );

    let status = render_status_output(
        &inspection,
        &ConnectionId::new("openai"),
        "gpt-5.4-mini",
        SessionToolMode::Extended,
    );
    for connection in &inspection.connections {
        assert!(status.contains(&format!(
            "- {} readiness={}",
            connection.connection_id,
            connection.readiness_label()
        )));
        assert!(actual.contains(&format!(
            "- {} ({}) readiness={}",
            connection.connection_id,
            connection.provider,
            connection.readiness_label()
        )));
    }

    let models = render_models_output(
        &connections,
        &backends,
        &report,
        &ConnectionId::new("openai"),
        "gpt-5.4-mini",
    );
    assert!(models.contains("chatgpt (openai-chatgpt)"));
}

#[test]
fn operator_surface_models_matches_fixture() {
    let (_, connections, backends, report, _, _) = sample_operator_surface_matrix();
    let actual = render_models_output(
        &connections,
        &backends,
        &report,
        &ConnectionId::new("openai"),
        "gpt-5.4-mini",
    );

    assert_operator_fixture("models.txt", &actual);
}

#[test]
fn operator_surface_session_inspect_matches_fixture() {
    let root = inspection_fixture_root();
    let response: SessionInspectionResponse = serde_json::from_value(
        root.get("session_inspection_response")
            .cloned()
            .expect("session inspection fixture"),
    )
    .expect("session inspection decode");
    let actual = render_session_output(&response.inspection);

    assert_operator_fixture("inspect-session.txt", &actual);
}

#[test]
fn operator_surface_execution_inspect_matches_fixture() {
    let root = inspection_fixture_root();
    let response: SessionExecutionResponse = serde_json::from_value(
        root.get("session_execution_response")
            .cloned()
            .expect("session execution fixture"),
    )
    .expect("session execution decode");
    let actual = render_execution_output(&response.inspection, 5);

    assert_operator_fixture("inspect-execution.txt", &actual);
}

#[test]
fn operator_surface_tool_call_inspection_matches_fixture() {
    let inspection = SessionToolCallInspection {
        session_id: "00000000-0000-0000-0000-000000000011"
            .parse()
            .expect("session id"),
        call_id: ToolCallId::new("call_shell"),
        tool_name: "shell".to_owned(),
        branch_id: Some(
            "20000000-0000-0000-0000-000000000011"
                .parse()
                .expect("branch id"),
        ),
        turn_id: Some(
            "30000000-0000-0000-0000-000000000011"
                .parse()
                .expect("turn id"),
        ),
        requested_seq_id: Some(7),
        completed_seq_id: Some(9),
        requested_at: Some(OffsetDateTime::UNIX_EPOCH),
        completed_at: Some(OffsetDateTime::UNIX_EPOCH),
        approval_status: Some("approved".to_owned()),
        approval_decision: Some(bt_core::ApprovalDecision::Approved {
            decided_at: OffsetDateTime::UNIX_EPOCH,
            decided_by: "operator".to_owned(),
            scope: bt_core::ApprovalScope::Session,
            source: bt_core::ApprovalDecisionSource::Human,
        }),
        approval_request_snapshot: None,
        approval_resolution: None,
        approval_updated_at: Some(OffsetDateTime::UNIX_EPOCH),
        execution_status: Some("completed".to_owned()),
        arguments: Some(json!({ "command": "cargo test -p bt-tui" })),
        result: Some(json!({ "stdout": "ok", "exit_code": 0 })),
    };
    let actual = render_tool_call_inspection_output(&inspection);

    assert_operator_fixture("inspect-tool-call.txt", &actual);
}

#[test]
fn operator_surface_approval_prompt_matches_fixture() {
    let mut app = test_app();
    let call_id = ToolCallId::new("call_shell");
    app.upsert_pending_tool(
        call_id.clone(),
        "shell".to_owned(),
        Some("cargo test -p bt-tui".to_owned()),
    );
    app.apply_queue_inspection(SessionQueueInspection {
        session_id: app.session_id,
        runtime_state: SessionRuntimeState::WaitingOnApproval,
        cancel_requested: false,
        pending_steer_count: 0,
        queued_messages: Vec::new(),
        pending_approvals: vec![PendingApprovalInspection {
            call_id,
            tool_name: "shell".to_owned(),
            branch_id: app.branch_id,
            turn_id: bt_core::TurnId::new(),
            settings_revision_id: default_settings_revision_id(),
            requested_at: OffsetDateTime::UNIX_EPOCH,
            request_snapshot: None,
        }],
        pending_inputs: Vec::new(),
    });

    let surface = render_bottom_panel(&app);
    assert_eq!(surface.kind, BottomSurfaceKind::Approval);
    assert!(surface.lines.len() <= 5);

    let actual = lines_to_text(&surface.lines);
    assert_operator_fixture("approval-prompt.txt", &actual);
}

#[test]
fn operator_surface_error_projection_matches_fixture() {
    let turn = bt_core::TurnInspection {
        turn_id: "30000000-0000-0000-0000-000000000021"
            .parse()
            .expect("turn id"),
        branch_id: "20000000-0000-0000-0000-000000000021"
            .parse()
            .expect("branch id"),
        provider: "openai".to_owned(),
        model: "gpt-5.4-mini".to_owned(),
        message_count: 1,
        settings_revision_id: default_settings_revision_id(),
        started_at: OffsetDateTime::UNIX_EPOCH,
        finished_at: Some(OffsetDateTime::UNIX_EPOCH),
        status: Some("failed".to_owned()),
        finish_reason: Some("error".to_owned()),
        latency_ms: Some(12),
        event_seq_start: Some(1),
        event_seq_end: Some(2),
        event_count: 2,
        pending_approval_count: 0,
        raw_chunk_count: 0,
        tool_calls: Vec::new(),
    };
    let events = vec![event(
        2,
        EventPayload::SessionError {
            class: ErrorClass::Runtime,
            code: "tool.shell_failed".to_owned(),
            message: "Shell command failed; retry with a smaller scope.".to_owned(),
            retryable: true,
        },
    )];

    let actual = render_raw_diff_output(&turn, &[], None, false, &events);
    assert_operator_fixture("error-render.txt", &actual);
}
