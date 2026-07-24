#![forbid(unsafe_code)]

mod app_state;
mod background_tasks;
mod bottom_pane;
mod command_actions;
mod command_render;
mod commands;
mod custom_terminal;
mod history_cell;
mod inline_terminal;
mod input_editor;
mod insert_history;
mod markdown;
mod markdown_render;
mod markdown_stream;
mod message_render;
mod pending_views;
mod runtime_loop;
mod session_loader;
mod session_state;
mod stream_state;
mod streaming;
mod text_utils;
mod transcript_helpers;
mod transcript_view;
mod user_surface;

use bottom_pane::{
    bottom_shell_height, composer_body_height, pending_chat_placeholder, popup_or_footer_height,
    render_bottom_surface, shell_aux_height, shell_aux_lines, shell_top_gap_height,
};
#[cfg(test)]
use bottom_pane::{render_bottom_panel, render_footer_lines};
use bt_client::BelltowerClient;
use bt_core::{
    ApprovalDecision, ApprovalScope, BelltowerConfig, BranchId, BranchRecord, CompletionDelta,
    ConnectionDescriptor, ConnectionId, ConnectionModelInventory, ConnectionModelSource,
    ConnectionReadinessInspection, EventEnvelope, EventId, EventPayload, McpServerDescriptor,
    McpToolDescriptor, Message, MessagePart, ModelBackendDescriptor, PendingApprovalInspection,
    PendingInputInspection, Role, SessionId, SessionQueueInspection, SessionRecord,
    SessionRuntimeState, SessionToolMode, StartupTrace, StatusInspection, ToolCallId,
    ToolResultEnvelope, TurnInspection, persist_connection_default_model,
    persist_default_connection,
};
use bt_protocol::{
    ActivateBranchRequest, AnswerToolRequest, ApproveToolRequest, CancelSessionRequest,
    CompactSessionRequest, CreateBranchRequest, CreateSessionRequest, ErrorEnvelope,
    PushOtlpExportRequest, RecordOperatorCommandRequest, RunShellCommandRequest,
    SendMessageOutcome, SendMessageRequest, SendMessageResponse, SpawnSessionRequest,
    SteerSessionRequest, UpdateSessionRequest,
};
use clap::{Args, Parser, Subcommand};
use command_render::{
    correlate_raw_diff_events, render_branches_output, render_compaction_output,
    render_defaults_output, render_doctor_output, render_execution_output, render_export_output,
    render_lineage_output, render_mcp_output, render_models_output, render_otlp_push_output,
    render_queue_clear_output, render_queue_output, render_raw_diff_output, render_raw_output,
    render_session_output, render_spawn_output, render_status_output,
    render_tool_call_inspection_output, render_tree_output, render_turn_history_output,
    render_usage_output, render_workflow_output, turn_event_matches,
};
#[cfg(test)]
use commands::session_readiness_summary;
use commands::{
    CommandMenuItem, RawCommandMode, approval_scope_label, command_allowed_during_request,
    command_catalog_line, command_help_output, command_menu_items, completion_candidates_line,
    completion_for_input, is_operator_shell_input, parse_approval_scope, parse_raw_command_args,
    resolve_command, resolve_raw_turn_selection, resolve_resume_selection, shared_prefix,
};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event as TerminalEvent, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use futures_util::StreamExt;
use history_cell::{
    HistoryCell, PlainHistoryCell, SharedHistoryCell, ToolCallHistoryCell,
    ToolExplorationHistoryCell, is_exploration_tool_name,
};
use inline_terminal::{
    TerminalGuard, create_inline_terminal, desired_inline_viewport_height, draw_chat,
    process_terminal_actions, sync_transcript_scrollback,
};
#[cfg(test)]
use message_render::render_message_with_options;
use message_render::{
    ask_answer_from_result, ask_question_from_call, assistant_markdown_source,
    message_is_ask_exchange_only, render_ask_history_entry, render_compact_operator_entry,
    render_message, render_tool_block_entry, render_transcript_message_with_options,
    transcript_entry_kind_for_message, transcript_separator_between, user_message_history_entry,
    visible_streaming_delta_text,
};
use pending_views::PendingToolView;
pub(crate) use pending_views::{pending_tool_calls, summarize_tool_detail};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use runtime_loop::{ChatExit, ChatOpenMode, run_chat};
use session_loader::{
    default_connection, load_full_transcript, load_session_state, load_transcript_page,
    matching_sessions, open_chat_session, resume_candidates, session_title, sorted_sessions,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::io::{self, Write};
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};
use streaming::chunking::AdaptiveChunkingPolicy;
use streaming::controller::StreamController;
pub(crate) use text_utils::{
    WrappedTextRow, display_project_root, display_width, next_char_boundary,
    next_stream_retry_delay, next_word_boundary, pad_to_display_width, parse_tool_mode,
    previous_char_boundary, previous_word_boundary, render_tool_mode, short_id_string,
    take_prefix_by_display_width, truncate_detail, truncate_path, wrap_composer_input,
    wrap_composer_input_with_end_indices, wrap_plain_text, wrapped_composer_cursor_position,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
pub(crate) use transcript_helpers::{
    errors_look_equivalent, merge_messages, merge_operator_commands, message_has_text,
    operator_command_print_key,
};
use transcript_view::{
    compare_transcript_seq_ids, merged_rendered_transcript_entries, render_operator_command,
};
pub(crate) use user_surface::{composer_text_area_rect, composer_wrap_width, user_surface_style};

const COMMAND_NOTICE_TTL: Duration = Duration::from_secs(5);
const METADATA_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const READINESS_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const MCP_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const FALLBACK_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const QUEUE_REFRESH_INTERVAL: Duration = Duration::from_millis(100);
const EVENT_STREAM_RETRY_MAX_DELAY: Duration = Duration::from_secs(5);
const DETACH_EXIT_CODE: i32 = 75;
const BOTTOM_PANEL_ACTIVE_HEIGHT: u16 = 7;
const MIN_INLINE_VIEWPORT_HEIGHT: u16 = 1;
const MIN_SCROLLBACK_HISTORY_HEIGHT: u16 = 6;
const TRANSCRIPT_PAGE_LIMIT: usize = 200;
const DEFAULT_RAW_CHUNK_LIMIT: usize = 40;
const RAW_CHUNK_PREVIEW_LIMIT: usize = 160;
const INPUT_PANEL_MIN_HEIGHT: u16 = 1;
const INPUT_PANEL_MAX_HEIGHT: u16 = 10;
const COMPOSER_GROWTH_HEADROOM: u16 = 0;
const COMPOSER_TOP_PADDING: u16 = 1;
const COMPOSER_BOTTOM_PADDING: u16 = 1;
const COMPOSER_RIGHT_PADDING: u16 = 1;
const MIN_COMPOSER_BODY_HEIGHT: u16 = 3;
const DEFAULT_TEXT_VIEW_WIDTH: u16 = 80;
const ESCAPE_PREFIX_TIMEOUT: Duration = Duration::from_millis(50);
const COMPOSER_BACKGROUND: Color = Color::Rgb(58, 58, 58);
const COMPOSER_PREFIX: &str = "› ";
const COMPOSER_CONTINUATION_PREFIX: &str = "  ";
const INPUT_PLACEHOLDER: &str = "Type to chat. Escape to exit. /help for commands";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TranscriptDensity {
    Normal,
    Verbose,
}

#[derive(Parser, Debug)]
#[command(name = "bt-tui")]
#[command(about = "Belltower protocol client")]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:7400/")]
    server: String,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    Chat(ChatArgs),
    Resume(ChatArgs),
    Health,
    Connections,
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    Models {
        #[command(subcommand)]
        command: ModelCommand,
    },
    Sessions {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Messages {
        session_id: SessionId,
    },
    Events {
        session_id: SessionId,
        #[arg(long)]
        last_event_id: Option<i64>,
        #[arg(long, default_value_t = 10)]
        max_events: usize,
    },
    Branches {
        #[command(subcommand)]
        command: BranchCommand,
    },
    RawChunks {
        session_id: SessionId,
    },
    Export {
        session_id: SessionId,
        #[arg(long, default_value = "html")]
        format: String,
    },
    Send {
        session_id: SessionId,
        branch_id: BranchId,
        text: String,
    },
}

#[derive(Args, Debug, Clone, Default)]
struct ChatArgs {
    #[arg(long)]
    session_id: Option<SessionId>,
    #[arg(long)]
    project_root: Option<String>,
    #[arg(long)]
    connection: Option<ConnectionId>,
    #[arg(long)]
    display_name: Option<String>,
    #[arg(long)]
    objective: Option<String>,
}

#[derive(Subcommand, Debug)]
enum BranchCommand {
    List {
        session_id: SessionId,
    },
    Create {
        session_id: SessionId,
        from_branch_id: BranchId,
        #[arg(long)]
        from_event_id: Option<EventId>,
        #[arg(long, default_value_t = true)]
        activate: bool,
        #[arg(long, default_value_t = true)]
        carry_summary: bool,
    },
    Activate {
        session_id: SessionId,
        branch_id: BranchId,
        #[arg(long, default_value_t = true)]
        carry_summary: bool,
    },
    Messages {
        session_id: SessionId,
        branch_id: BranchId,
    },
}

#[derive(Subcommand, Debug)]
enum SessionCommand {
    List,
    Create {
        project_root: String,
        #[arg(long)]
        connection: Option<ConnectionId>,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long)]
        objective: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum McpCommand {
    Servers,
    Tools,
    Reload,
}

#[derive(Subcommand, Debug)]
enum ModelCommand {
    Backends,
    Recommendations,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let client = BelltowerClient::try_from(cli.server.as_str())?;

    match cli.command.unwrap_or(Command::Chat(ChatArgs::default())) {
        Command::Chat(args) => {
            if run_chat(client, args, ChatOpenMode::New).await? == ChatExit::Detached {
                std::process::exit(DETACH_EXIT_CODE);
            }
        }
        Command::Resume(args) => {
            if run_chat(client, args, ChatOpenMode::ResumePicker).await? == ChatExit::Detached {
                std::process::exit(DETACH_EXIT_CODE);
            }
        }
        Command::Health => {
            let response = client.health().await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        Command::Connections => {
            let response = client.connections().await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        Command::Mcp { command } => match command {
            McpCommand::Servers => {
                let response = client.mcp_servers().await?;
                println!("{}", serde_json::to_string_pretty(&response)?);
            }
            McpCommand::Tools => {
                let response = client.mcp_tools().await?;
                println!("{}", serde_json::to_string_pretty(&response)?);
            }
            McpCommand::Reload => {
                client.reload_mcp().await?;
                println!("reloaded MCP servers");
            }
        },
        Command::Models { command } => match command {
            ModelCommand::Backends => {
                let response = client.model_backends().await?;
                println!("{}", serde_json::to_string_pretty(&response)?);
            }
            ModelCommand::Recommendations => {
                let response = client.model_recommendations().await?;
                println!("{}", serde_json::to_string_pretty(&response)?);
            }
        },
        Command::Sessions { command } => match command {
            SessionCommand::List => {
                let response = client.list_sessions().await?;
                println!("{}", serde_json::to_string_pretty(&response)?);
            }
            SessionCommand::Create {
                project_root,
                connection,
                display_name,
                objective,
            } => {
                let connection = connection.unwrap_or(default_connection()?);
                let response = client
                    .create_session(&CreateSessionRequest {
                        project_root,
                        connection_id: connection,
                        model_id: None,
                        tool_mode: None,
                        display_name,
                        objective,
                        budget: None,
                        approval_mode: None,
                    })
                    .await?;
                println!("{}", serde_json::to_string_pretty(&response)?);
            }
        },
        Command::Messages { session_id } => {
            let response = client.session_messages(session_id).await?;
            for message in response.messages {
                println!("{}", render_message(&message));
            }
        }
        Command::Events {
            session_id,
            last_event_id,
            max_events,
        } => {
            let mut stream = client.stream_events(session_id, last_event_id).await?;
            let mut seen = 0usize;
            while seen < max_events {
                let Some(event) = stream.next().await else {
                    break;
                };
                println!("{}", serde_json::to_string_pretty(&event?)?);
                seen += 1;
            }
        }
        Command::Branches { command } => match command {
            BranchCommand::List { session_id } => {
                let response = client.branches(session_id).await?;
                println!("{}", serde_json::to_string_pretty(&response)?);
            }
            BranchCommand::Create {
                session_id,
                from_branch_id,
                from_event_id,
                activate,
                carry_summary,
            } => {
                let response = client
                    .create_branch(
                        session_id,
                        &CreateBranchRequest {
                            from_branch_id,
                            from_event_id,
                            activate,
                            carry_summary,
                        },
                    )
                    .await?;
                println!("{}", serde_json::to_string_pretty(&response)?);
            }
            BranchCommand::Activate {
                session_id,
                branch_id,
                carry_summary,
            } => {
                client
                    .activate_branch(
                        session_id,
                        branch_id,
                        &ActivateBranchRequest { carry_summary },
                    )
                    .await?;
                let response = client.branch_messages(session_id, branch_id).await?;
                for message in response.messages {
                    println!("{}", render_message(&message));
                }
            }
            BranchCommand::Messages {
                session_id,
                branch_id,
            } => {
                let response = client.branch_messages(session_id, branch_id).await?;
                for message in response.messages {
                    println!("{}", render_message(&message));
                }
            }
        },
        Command::RawChunks { session_id } => {
            let response = client.raw_chunks(session_id).await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        Command::Export { session_id, format } => {
            let response = client.export_session(session_id, &format).await?;
            println!("{}", response.content);
        }
        Command::Send {
            session_id,
            branch_id,
            text,
        } => {
            let response = client
                .send_message(
                    session_id,
                    &SendMessageRequest {
                        branch_id,
                        message: Message::text(Role::User, text),
                    },
                )
                .await?;
            match response.outcome {
                SendMessageOutcome::Dispatched => {
                    let response = client.session_messages(session_id).await?;
                    for message in response.messages {
                        println!("{}", render_message(&message));
                    }
                }
                SendMessageOutcome::Queued { position } => {
                    println!("Message queued at position {position}.");
                }
            }
        }
    }

    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RecordedOperatorCommand {
    seq_id: Option<i64>,
    occurred_at: time::OffsetDateTime,
    command_type: String,
    raw_input: String,
    output: String,
    success: bool,
}

#[derive(Clone, Debug)]
struct LoadedTranscriptPage {
    messages: Vec<Message>,
    message_seq_ids: Vec<Option<i64>>,
    operator_commands: Vec<RecordedOperatorCommand>,
    has_more_before: bool,
    last_event_id: Option<i64>,
}

#[derive(Clone, Debug, Default)]
struct ComposerState {
    input: String,
    cursor: usize,
    preferred_column: Option<usize>,
    kill_buffer: String,
    history_browse_index: Option<usize>,
    history_browse_draft: Option<String>,
    view_width: u16,
}

#[derive(Clone, Debug)]
struct BottomShellState {
    command_menu_selection: usize,
    command_menu_scroll_top: usize,
    approval_menu_selection: usize,
    question_choice_selection: usize,
    bottom_panel_view_width: u16,
}

impl Default for BottomShellState {
    fn default() -> Self {
        Self {
            command_menu_selection: 0,
            command_menu_scroll_top: 0,
            approval_menu_selection: 0,
            question_choice_selection: 0,
            bottom_panel_view_width: DEFAULT_TEXT_VIEW_WIDTH,
        }
    }
}

#[derive(Default)]
struct ActiveTurnState {
    live: bool,
    started_at: Option<Instant>,
    needs_final_separator: bool,
    had_work_activity: bool,
    final_separator_emitted: bool,
    message_start: Option<usize>,
    command_start: Option<usize>,
    cells: VecDeque<ActiveCell>,
    revision: u64,
    stream_controller: Option<StreamController>,
    streaming_tool_arguments: BTreeMap<ToolCallId, String>,
    committed_tool_result_call_ids: BTreeSet<ToolCallId>,
    committed_assistant_text: String,
    deferred_streaming_assistant_text: String,
    deferred_canonical_assistant_text: Option<String>,
    optimistic_user_text: Option<String>,
}

#[derive(Clone, Debug)]
struct TaskStatusState {
    header: String,
    detail: Option<String>,
    show_interrupt_hint: bool,
}

struct ChatApp {
    client: BelltowerClient,
    session_id: SessionId,
    branch_id: BranchId,
    connection_id: ConnectionId,
    connection_model: Option<String>,
    tool_mode: SessionToolMode,
    project_root: String,
    messages: Vec<Message>,
    message_seq_ids: Vec<Option<i64>>,
    operator_commands: Vec<RecordedOperatorCommand>,
    sessions: Vec<SessionRecord>,
    branches: Vec<BranchRecord>,
    connections: Vec<ConnectionDescriptor>,
    connection_status: Option<StatusInspection>,
    connection_models: Vec<ConnectionModelInventory>,
    model_backends: Vec<ModelBackendDescriptor>,
    mcp_servers: Vec<McpServerDescriptor>,
    mcp_tools: Vec<McpToolDescriptor>,
    pending_tools: Vec<PendingToolView>,
    pending_approvals: Vec<PendingApprovalInspection>,
    pending_inputs: Vec<PendingInputInspection>,
    queue_inspection: Option<SessionQueueInspection>,
    queue_refresh_requested: bool,
    resolving_tools: BTreeSet<ToolCallId>,
    composer: ComposerState,
    bottom_shell: BottomShellState,
    status: String,
    task_status: Option<TaskStatusState>,
    transient_status: Option<String>,
    transient_status_until: Option<Instant>,
    pending_escape_prefix_at: Option<Instant>,
    message_view_height: u16,
    message_view_width: u16,
    transcript_output_width: u16,
    show_reasoning: bool,
    auto_follow_messages: bool,
    resume_follow_lock: bool,
    post_resume_layout_sync_pending: bool,
    pending_hard_clear: bool,
    pending_visible_clear: bool,
    pending_scrollback_reset_text: Option<String>,
    pending_scrollback_reset_lines: Option<Vec<Line<'static>>>,
    should_quit: bool,
    detach_requested: bool,
    pending_send: Option<JoinHandle<std::result::Result<PendingSendCompletion, String>>>,
    pending_send_started_at: Option<Instant>,
    pending_message_submissions: VecDeque<PendingMessageSubmission>,
    pending_session_load: Option<JoinHandle<std::result::Result<LoadedSessionState, String>>>,
    pending_resume_command_raw_input: Option<String>,
    pending_history_backfill: Option<JoinHandle<std::result::Result<LoadedTranscriptPage, String>>>,
    pending_shell_command: Option<String>,
    pending_approval: Option<JoinHandle<std::result::Result<(), String>>>,
    pending_approval_started_at: Option<Instant>,
    pending_approval_context: Option<PendingApprovalAction>,
    cancel_requested: bool,
    stream_task: Option<JoinHandle<()>>,
    stream_updates: Option<mpsc::UnboundedReceiver<ChatStreamUpdate>>,
    last_event_id: Option<i64>,
    stream_retry_at: Option<Instant>,
    stream_retry_attempt: u8,
    active_turn: ActiveTurnState,
    adaptive_chunking: AdaptiveChunkingPolicy,
    assistant_commit_batch_size: Option<usize>,
    pending_history_entries: VecDeque<HistoryEntry>,
    committed_history_entries: Vec<HistoryEntry>,
    printed_message_ids: BTreeSet<String>,
    printed_tool_call_ids: BTreeSet<String>,
    printed_operator_command_keys: BTreeSet<String>,
    has_more_history_before: bool,
    last_refresh: Instant,
    last_metadata_refresh: Instant,
    last_queue_refresh: Instant,
    last_readiness_refresh: Instant,
    last_mcp_refresh: Instant,
}

enum ChatStreamUpdate {
    Event(Box<EventEnvelope>),
    Error(String),
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScrollbackSeparator {
    Paragraph,
    Line,
    None,
}

#[derive(Clone)]
struct HistoryEntry {
    cell: SharedHistoryCell,
    separator: ScrollbackSeparator,
    kind: Option<TranscriptEntryKind>,
}

#[derive(Clone, Debug)]
enum ActiveCell {
    Tool(ToolCallHistoryCell),
    Exploration(ToolExplorationHistoryCell),
}

impl ActiveCell {
    fn into_shared(self) -> SharedHistoryCell {
        match self {
            Self::Tool(cell) => Arc::new(cell),
            Self::Exploration(cell) => Arc::new(cell),
        }
    }

    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        match self {
            Self::Tool(cell) => cell.display_lines(width),
            Self::Exploration(cell) => cell.display_lines(width),
        }
    }

    fn desired_height(&self, width: u16) -> u16 {
        match self {
            Self::Tool(cell) => cell.desired_height(width),
            Self::Exploration(cell) => cell.desired_height(width),
        }
    }

    fn is_ready_to_flush(&self) -> bool {
        match self {
            Self::Tool(cell) => cell.result.is_some(),
            Self::Exploration(_) => false,
        }
    }

    fn is_complete(&self) -> bool {
        match self {
            Self::Tool(cell) => cell.result.is_some(),
            Self::Exploration(cell) => cell.is_complete(),
        }
    }

    fn contains_call_id(&self, call_id: &ToolCallId) -> bool {
        match self {
            Self::Tool(cell) => cell.call_id == call_id.to_string(),
            Self::Exploration(cell) => cell.contains_call_id(&call_id.to_string()),
        }
    }

    fn visible_in_live_view(&self) -> bool {
        match self {
            Self::Tool(_) => true,
            Self::Exploration(_) => true,
        }
    }

    fn call_ids(&self) -> Vec<String> {
        match self {
            Self::Tool(cell) => vec![cell.call_id.clone()],
            Self::Exploration(cell) => cell.call_ids(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TranscriptEntryKind {
    User,
    Assistant,
    ToolCall,
    ToolResult,
    Operator,
    LocalError,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingApprovalAction {
    pending: PendingToolView,
}

#[derive(Clone, Debug)]
struct LoadedSessionState {
    session: SessionRecord,
    branch_id: BranchId,
    messages: Vec<Message>,
    message_seq_ids: Vec<Option<i64>>,
    operator_commands: Vec<RecordedOperatorCommand>,
    has_more_history_before: bool,
    sessions: Vec<SessionRecord>,
    branches: Vec<BranchRecord>,
    connections: Vec<ConnectionDescriptor>,
    connection_model: Option<String>,
    connection_models: Vec<ConnectionModelInventory>,
    last_event_id: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ApprovalMenuChoice {
    ApproveOnce,
    ApproveSession,
    ApproveAlways,
    Deny,
}

impl ApprovalMenuChoice {
    fn all() -> &'static [Self] {
        &[
            Self::ApproveOnce,
            Self::ApproveSession,
            Self::ApproveAlways,
            Self::Deny,
        ]
    }

    fn label(self) -> &'static str {
        match self {
            Self::ApproveOnce => "Approve once",
            Self::ApproveSession => "Approve for session",
            Self::ApproveAlways => "Approve always",
            Self::Deny => "Deny",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::ApproveOnce => "Run this tool call and ask again next time",
            Self::ApproveSession => "Allow matching calls for the rest of this session",
            Self::ApproveAlways => "Allow matching calls for the current server runtime",
            Self::Deny => "Reject this tool call",
        }
    }

    fn to_chat_action(self) -> ChatAction {
        match self {
            Self::ApproveOnce => ChatAction::ApproveLastPendingOnce,
            Self::ApproveSession => ChatAction::ApproveLastPendingSession,
            Self::ApproveAlways => ChatAction::ApproveLastPendingAlways,
            Self::Deny => ChatAction::DenyLastPending,
        }
    }
}

#[derive(Debug)]
struct PendingMessageSubmission {
    preview: String,
    handle: JoinHandle<std::result::Result<SendMessageResponse, String>>,
}

#[derive(Debug)]
enum PendingSendCompletion {
    MessageSubmitted(SendMessageResponse),
    Ack,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChatAction {
    Send,
    Refresh,
    Cancel,
    ApproveLastPendingOnce,
    ApproveLastPendingSession,
    ApproveLastPendingAlways,
    DenyLastPending,
    PreviousSession,
    NextSession,
    NewSession,
    CreateBranch,
    PreviousBranch,
    NextBranch,
}

#[cfg(test)]
mod tests;
