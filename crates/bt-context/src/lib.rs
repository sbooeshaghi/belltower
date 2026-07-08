#![forbid(unsafe_code)]

use bt_core::{
    BelltowerConfig, CompletionRequest, ConnectionId, ContextCompactionTrigger,
    ContextWindowCatalog, InstructionDocument, Message, MessageId, MessagePart, PlanInspection,
    PlanStatus, Result, Role, SessionRecord, ThinkingConfig, ThinkingEffort, ToolSpec,
};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::fs;

#[derive(Clone, Debug)]
pub struct ContextAssembler {
    config: BelltowerConfig,
    windows: ContextWindowCatalog,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextBuildOutput {
    pub request: CompletionRequest,
    pub compaction: Option<ContextCompaction>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextCompaction {
    pub summary: String,
    pub trigger: ContextCompactionTrigger,
    pub reason: Option<String>,
    pub summary_message_id: Option<MessageId>,
    pub first_kept_message_id: Option<MessageId>,
    pub messages_before: u32,
    pub messages_after: u32,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub files_read: Vec<String>,
    pub files_modified: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BranchSummary {
    pub summary: String,
    pub files_read: Vec<String>,
    pub files_modified: Vec<String>,
}

#[derive(Clone, Debug)]
struct MessageUnit {
    messages: Vec<Message>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FileActivity {
    files_read: BTreeSet<String>,
    files_modified: BTreeSet<String>,
}

#[derive(Clone, Debug)]
pub struct SystemPromptBuilder;

#[derive(Clone, Debug)]
pub struct SystemPromptInput<'a> {
    pub session: &'a SessionRecord,
    pub provider_family: &'a str,
    pub core_prompt: &'a InstructionDocument,
    pub provider_overlay: Option<&'a InstructionDocument>,
    pub instructions: &'a [InstructionDocument],
    pub plan: Option<&'a PlanInspection>,
    pub tools: &'a [ToolSpec],
}

impl ContextAssembler {
    pub fn new(config: BelltowerConfig) -> Result<Self> {
        Ok(Self {
            config,
            windows: BelltowerConfig::context_window_catalog()?,
        })
    }

    pub fn build_request(
        &self,
        connection_id: ConnectionId,
        provider: &str,
        model: &str,
        system_prompt: Option<String>,
        messages: Vec<Message>,
        tools: Vec<ToolSpec>,
        thinking: Option<ThinkingConfig>,
    ) -> CompletionRequest {
        self.build_request_with_metadata(
            connection_id,
            provider,
            model,
            system_prompt,
            messages,
            tools,
            thinking,
        )
        .request
    }

    pub fn build_request_with_options(
        &self,
        connection_id: ConnectionId,
        provider: &str,
        model: &str,
        system_prompt: Option<String>,
        messages: Vec<Message>,
        tools: Vec<ToolSpec>,
        thinking: Option<ThinkingConfig>,
        force_compaction: bool,
    ) -> ContextBuildOutput {
        let max_prompt_tokens = self
            .context_window_for(provider, model)
            .saturating_sub(self.config.context.reserve_tokens);
        let system_tokens = system_prompt
            .as_deref()
            .map(estimate_text_tokens)
            .unwrap_or(0);
        let normalized_messages = normalize_messages(
            messages,
            self.config.context.max_tool_result_lines,
            self.config.context.max_tool_result_bytes,
        );
        let tokens_before =
            estimate_messages_tokens(&normalized_messages).saturating_add(system_tokens);

        let (selected_messages, compaction) =
            if force_compaction || tokens_before > max_prompt_tokens {
                let trigger = if force_compaction {
                    ContextCompactionTrigger::Forced
                } else {
                    ContextCompactionTrigger::TokenBudget
                };
                compact_messages(
                    normalized_messages,
                    max_prompt_tokens.saturating_sub(system_tokens),
                    force_compaction,
                    trigger,
                )
            } else {
                (normalized_messages, None)
            };

        let tokens_after =
            estimate_messages_tokens(&selected_messages).saturating_add(system_tokens);
        let compaction = compaction.map(|mut compaction| {
            compaction.tokens_before = tokens_before;
            compaction.tokens_after = tokens_after;
            compaction.messages_after = selected_messages.len() as u32;
            compaction
        });
        let thinking = effective_thinking_config(
            &connection_id,
            provider,
            model,
            self.config.context.reserve_tokens,
            thinking,
        );

        ContextBuildOutput {
            request: CompletionRequest {
                connection_id,
                model: model.to_owned(),
                system_prompt,
                messages: selected_messages,
                tools,
                structured_output: None,
                max_tokens: Some(self.config.context.reserve_tokens),
                temperature: None,
                thinking,
            },
            compaction,
        }
    }

    pub fn build_request_with_metadata(
        &self,
        connection_id: ConnectionId,
        provider: &str,
        model: &str,
        system_prompt: Option<String>,
        messages: Vec<Message>,
        tools: Vec<ToolSpec>,
        thinking: Option<ThinkingConfig>,
    ) -> ContextBuildOutput {
        self.build_request_with_options(
            connection_id,
            provider,
            model,
            system_prompt,
            messages,
            tools,
            thinking,
            false,
        )
    }

    #[must_use]
    pub fn context_window_for(&self, provider: &str, model: &str) -> u64 {
        self.windows
            .window
            .iter()
            .find(|entry| entry.provider == provider && model.contains(&entry.model_pattern))
            .map(|entry| entry.context_window_tokens)
            .unwrap_or(32_768)
    }
}

fn effective_thinking_config(
    connection_id: &ConnectionId,
    provider: &str,
    model: &str,
    reserve_tokens: u64,
    requested: Option<ThinkingConfig>,
) -> Option<ThinkingConfig> {
    if requested.is_some() {
        return requested;
    }

    if provider == "openai-chatgpt" && openai_reasoning_effort(model).is_some() {
        return Some(ThinkingConfig {
            enabled: true,
            effort: openai_reasoning_effort(model),
            budget_tokens: None,
            include_summaries: true,
        });
    }

    if provider == "openai-compatible"
        && connection_id.0 == "openai"
        && openai_reasoning_effort(model).is_some()
    {
        return Some(ThinkingConfig {
            enabled: true,
            effort: openai_reasoning_effort(model),
            budget_tokens: None,
            include_summaries: true,
        });
    }

    if provider == "anthropic" && supports_anthropic_thinking(model) {
        return Some(ThinkingConfig {
            enabled: true,
            effort: Some(ThinkingEffort::High),
            budget_tokens: Some(default_anthropic_thinking_budget(reserve_tokens)),
            include_summaries: true,
        });
    }

    None
}

fn openai_reasoning_effort(model: &str) -> Option<ThinkingEffort> {
    let model = model.to_ascii_lowercase();
    if model.starts_with("o1") || model.starts_with("o3") || model.starts_with("o4") {
        return Some(ThinkingEffort::High);
    }
    if !model.starts_with("gpt-5") {
        return None;
    }
    if model.contains("-pro") {
        return Some(ThinkingEffort::High);
    }
    if model.starts_with("gpt-5.1-codex-max") {
        return Some(ThinkingEffort::XHigh);
    }
    if model.starts_with("gpt-5.1") {
        return Some(ThinkingEffort::High);
    }
    Some(ThinkingEffort::XHigh)
}

fn supports_anthropic_thinking(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.starts_with("claude-opus-4")
        || model.starts_with("claude-sonnet-4")
        || model.starts_with("claude-haiku-4")
}

fn default_anthropic_thinking_budget(reserve_tokens: u64) -> u64 {
    let upper = reserve_tokens.saturating_sub(1);
    if upper < 1_024 {
        upper.max(1)
    } else {
        reserve_tokens
            .saturating_sub(1_024)
            .max(1_024)
            .min(32_768)
            .min(upper)
    }
}

impl SystemPromptBuilder {
    #[must_use]
    pub fn build(input: SystemPromptInput<'_>) -> String {
        let mut sections = vec![input.core_prompt.body.trim().to_owned()];

        if let Some(objective) = &input.session.objective
            && !objective.trim().is_empty()
        {
            sections.push(format!("Objective:\n{}", objective.trim()));
        }

        sections.push(render_runtime_context(input.session));

        if let Some(tool_guidance) = render_tool_guidance(input.tools) {
            sections.push(tool_guidance);
        }

        if let Some(overlay) = input.provider_overlay
            && !overlay.body.trim().is_empty()
        {
            sections.push(format!(
                "Provider notes ({provider}):\n{}",
                overlay.body.trim(),
                provider = input.provider_family
            ));
        }

        if !input.instructions.is_empty() {
            sections.push(render_instruction_documents(input.instructions));
        }

        if let Some(plan) = input.plan
            && !plan.items.is_empty()
        {
            sections.push(render_active_plan(plan));
        }

        sections.join("\n\n")
    }
}

fn render_runtime_context(session: &SessionRecord) -> String {
    let mut lines = vec![
        "Runtime context:".to_owned(),
        format!("- cwd: {}", session.project_root),
        format!("- connection: {}", session.connection_id),
    ];

    if let Some(model_id) = &session.model_id
        && !model_id.trim().is_empty()
    {
        lines.push(format!("- model: {model_id}"));
    }

    lines.push(format!(
        "- tool mode: {}",
        match session.tool_mode {
            bt_core::SessionToolMode::Standard => "standard",
            bt_core::SessionToolMode::Extended => "extended",
        }
    ));
    lines.push("- shell commands run from this cwd/project root.".to_owned());

    let validation_surfaces = validation_surfaces_for_project(&session.project_root);
    if !validation_surfaces.is_empty() {
        lines.push("- validation surfaces detected:".to_owned());
        for surface in validation_surfaces {
            lines.push(format!("  - {surface}"));
        }
    }

    lines.join("\n")
}

const MAX_VALIDATION_SURFACES: usize = 12;
const MAX_VALIDATION_SCAN_DEPTH: usize = 3;
const MAX_VALIDATION_SCAN_ENTRIES: usize = 256;

#[must_use]
pub fn validation_surfaces_for_project(project_root: &camino::Utf8Path) -> Vec<String> {
    let mut surfaces = BTreeSet::new();
    let mut visited_entries = 0;
    scan_validation_surfaces(
        project_root,
        project_root,
        0,
        &mut visited_entries,
        &mut surfaces,
    );
    surfaces.into_iter().take(MAX_VALIDATION_SURFACES).collect()
}

fn scan_validation_surfaces(
    project_root: &camino::Utf8Path,
    dir: &camino::Utf8Path,
    depth: usize,
    visited_entries: &mut usize,
    surfaces: &mut BTreeSet<String>,
) {
    if depth > MAX_VALIDATION_SCAN_DEPTH || *visited_entries >= MAX_VALIDATION_SCAN_ENTRIES {
        return;
    }

    let Ok(entries) = fs::read_dir(dir.as_std_path()) else {
        return;
    };
    let mut entries = entries.filter_map(|entry| entry.ok()).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_string());

    for entry in entries {
        if *visited_entries >= MAX_VALIDATION_SCAN_ENTRIES {
            break;
        }
        *visited_entries += 1;

        let name = entry.file_name().to_string_lossy().to_string();
        if should_skip_validation_scan_entry(&name) {
            continue;
        }

        let path = entry.path();
        let Ok(path) = camino::Utf8PathBuf::from_path_buf(path) else {
            continue;
        };
        let is_dir = entry
            .file_type()
            .map(|file_type| file_type.is_dir())
            .unwrap_or(false);

        if is_validation_surface_name(&name, is_dir)
            && let Some(surface) = relative_surface_path(project_root, &path, is_dir)
        {
            surfaces.insert(surface);
        }

        if is_dir && should_descend_validation_scan(&name, depth) {
            scan_validation_surfaces(project_root, &path, depth + 1, visited_entries, surfaces);
        }
    }
}

fn relative_surface_path(
    project_root: &camino::Utf8Path,
    path: &camino::Utf8Path,
    is_dir: bool,
) -> Option<String> {
    let relative = path.strip_prefix(project_root).ok()?;
    let mut rendered = relative.as_str().to_owned();
    if rendered.is_empty() {
        return None;
    }
    if is_dir && !rendered.ends_with('/') {
        rendered.push('/');
    }
    Some(rendered)
}

fn should_skip_validation_scan_entry(name: &str) -> bool {
    matches!(
        name,
        ".git" | ".belltower" | "target" | "node_modules" | ".venv" | "venv" | "__pycache__"
    ) || name.starts_with('.')
}

fn should_descend_validation_scan(name: &str, depth: usize) -> bool {
    if depth == 0 {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "test"
            | "tests"
            | "spec"
            | "specs"
            | "fixture"
            | "fixtures"
            | "schema"
            | "schemas"
            | "expected"
            | "golden"
    )
}

fn is_validation_surface_name(name: &str, is_dir: bool) -> bool {
    let lower = name.to_ascii_lowercase();
    if is_dir {
        return matches!(
            lower.as_str(),
            "test"
                | "tests"
                | "spec"
                | "specs"
                | "fixture"
                | "fixtures"
                | "schema"
                | "schemas"
                | "expected"
                | "golden"
        );
    }

    matches!(
        lower.as_str(),
        "agents.md" | "readme.md" | "makefile" | "justfile" | "pyproject.toml" | "package.json"
    ) || lower.contains("test")
        || lower.contains("spec")
        || lower.contains("schema")
        || lower.contains("fixture")
        || lower.contains("expected")
        || lower.contains("golden")
}

fn render_tool_guidance(tools: &[ToolSpec]) -> Option<String> {
    const TOOL_GUIDANCE_ORDER: [(&str, &str); 11] = [
        ("search", "discover files or matching content"),
        ("read", "inspect a known file"),
        ("list", "inspect directory structure"),
        ("edit", "make deliberate local file changes"),
        ("write", "create or replace a file intentionally"),
        (
            "shell",
            "use only when the structured tools are not the right fit",
        ),
        (
            "web_search",
            "discover external web sources before fetching specific URLs",
        ),
        (
            "web_fetch",
            "retrieve public URL content with source and truncation metadata",
        ),
        ("plan", "track multi-step work explicitly when it helps"),
        (
            "ask",
            "pause and ask the human when required input is missing",
        ),
        ("inspect", "check prior harness state instead of guessing"),
    ];

    let tool_names = tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut lines = Vec::new();

    for (name, description) in TOOL_GUIDANCE_ORDER {
        if tool_names.contains(name) {
            lines.push(format!("- `{name}`: {description}"));
        }
    }

    if tool_names.contains("catalogue") {
        lines.push(
            "- `catalogue`: discover additional external tools when the visible set is not enough."
                .to_owned(),
        );
    }

    (!lines.is_empty()).then(|| format!("Tool guidance:\n{}", lines.join("\n")))
}

fn render_instruction_documents(instructions: &[InstructionDocument]) -> String {
    instructions
        .iter()
        .map(|doc| format!("## {}\n{}", doc.title, doc.body.trim()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_active_plan(plan: &PlanInspection) -> String {
    let items = plan
        .items
        .iter()
        .map(|item| {
            format!(
                "- [{}] {}: {}",
                render_plan_status(&item.status),
                item.id,
                item.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("Active plan:\n{items}")
}

fn render_plan_status(status: &PlanStatus) -> &'static str {
    match status {
        PlanStatus::Pending => "pending",
        PlanStatus::InProgress => "in_progress",
        PlanStatus::Completed => "completed",
        PlanStatus::Blocked => "blocked",
    }
}

pub fn summarize_messages(messages: &[Message], max_tokens: u64) -> Option<BranchSummary> {
    let normalized = normalize_messages(messages.to_vec(), 2_000, 51_200);
    let file_activity = collect_file_activity(&normalized);
    let summary = build_summary(&normalized, &file_activity, max_tokens)?;
    Some(BranchSummary {
        summary,
        files_read: file_activity.files_read.into_iter().collect(),
        files_modified: file_activity.files_modified.into_iter().collect(),
    })
}

fn compact_messages(
    messages: Vec<Message>,
    max_prompt_tokens: u64,
    force_compaction: bool,
    trigger: ContextCompactionTrigger,
) -> (Vec<Message>, Option<ContextCompaction>) {
    let (pinned_prefix, remainder) = split_system_prefix(messages);
    let prefix_tokens = estimate_messages_tokens(&pinned_prefix);
    let mut retained_units = build_message_units(remainder);
    let mut dropped_units = Vec::new();

    if force_compaction && retained_units.len() > 1 {
        dropped_units.push(retained_units.remove(0));
    }

    loop {
        let retained_messages = flatten_units(&retained_units);
        let available_summary_tokens = max_prompt_tokens
            .saturating_sub(prefix_tokens)
            .saturating_sub(estimate_messages_tokens(&retained_messages));
        let dropped_messages = flatten_units(&dropped_units);
        let file_activity = collect_file_activity(&dropped_messages);
        let summary = build_summary(&dropped_messages, &file_activity, available_summary_tokens);

        let mut selected_messages = pinned_prefix.clone();
        let first_kept_message_id = retained_messages.first().map(|message| message.message_id);
        let messages_before_count =
            (pinned_prefix.len() + retained_messages.len() + dropped_messages.len()) as u32;
        let mut summary_message_id = None;
        if let Some(summary) = &summary {
            let summary_message = Message::text(Role::System, summary.clone());
            summary_message_id = Some(summary_message.message_id);
            selected_messages.push(summary_message);
        }
        selected_messages.extend(retained_messages);

        if estimate_messages_tokens(&selected_messages) <= max_prompt_tokens
            || retained_units.is_empty()
        {
            let compaction = summary.map(|summary| ContextCompaction {
                summary,
                trigger,
                reason: Some(match trigger {
                    ContextCompactionTrigger::Forced => "forced".to_owned(),
                    ContextCompactionTrigger::TokenBudget => "token_budget".to_owned(),
                }),
                summary_message_id,
                first_kept_message_id,
                messages_before: messages_before_count,
                messages_after: 0,
                tokens_before: 0,
                tokens_after: 0,
                files_read: file_activity.files_read.into_iter().collect(),
                files_modified: file_activity.files_modified.into_iter().collect(),
            });
            return (selected_messages, compaction);
        }

        dropped_units.push(retained_units.remove(0));
    }
}

fn split_system_prefix(messages: Vec<Message>) -> (Vec<Message>, Vec<Message>) {
    let split_index = messages
        .iter()
        .position(|message| message.role != Role::System)
        .unwrap_or(messages.len());
    let mut messages = messages;
    let remainder = messages.split_off(split_index);
    (messages, remainder)
}

fn build_message_units(messages: Vec<Message>) -> Vec<MessageUnit> {
    let mut units = Vec::new();
    let mut index = 0usize;
    while index < messages.len() {
        if let Some(unit) = tool_pair_unit(&messages, index) {
            index += unit.messages.len();
            units.push(unit);
            continue;
        }

        units.push(MessageUnit {
            messages: vec![messages[index].clone()],
        });
        index += 1;
    }
    units
}

fn tool_pair_unit(messages: &[Message], index: usize) -> Option<MessageUnit> {
    let current = messages.get(index)?;
    let next = messages.get(index + 1)?;
    let call = current.tool_call()?;
    let result = next.tool_result()?;
    if result.call_id.to_string() != call.call_id {
        return None;
    }

    Some(MessageUnit {
        messages: vec![current.clone(), next.clone()],
    })
}

fn flatten_units(units: &[MessageUnit]) -> Vec<Message> {
    units
        .iter()
        .flat_map(|unit| unit.messages.iter().cloned())
        .collect()
}

fn normalize_messages(messages: Vec<Message>, max_lines: usize, max_bytes: usize) -> Vec<Message> {
    messages
        .into_iter()
        .map(|mut message| {
            for part in &mut message.parts {
                if let MessagePart::ToolResult { result } = part {
                    result.output = truncate_tool_output(&result.output, max_lines, max_bytes);
                }
            }
            message
        })
        .collect()
}

fn truncate_tool_output(output: &Value, max_lines: usize, max_bytes: usize) -> Value {
    match output {
        Value::Object(map) => {
            let mut truncated = Map::new();
            for (key, value) in map {
                let next = if matches!(key.as_str(), "content" | "stdout" | "stderr") {
                    match value {
                        Value::String(text) => {
                            Value::String(truncate_text(text, max_lines, max_bytes))
                        }
                        other => other.clone(),
                    }
                } else {
                    value.clone()
                };
                truncated.insert(key.clone(), next);
            }
            Value::Object(truncated)
        }
        Value::String(text) => Value::String(truncate_text(text, max_lines, max_bytes)),
        other => other.clone(),
    }
}

fn truncate_text(text: &str, max_lines: usize, max_bytes: usize) -> String {
    let mut lines = text.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    if lines.len() > max_bytes {
        lines.truncate(max_bytes);
    }
    if lines != text {
        lines.push_str("\n...[truncated]");
    }
    lines
}

fn collect_file_activity(messages: &[Message]) -> FileActivity {
    let mut activity = FileActivity::default();
    for message in messages {
        let Some(result) = message.tool_result() else {
            continue;
        };
        match result.tool_name.as_str() {
            "read" => {
                if let Some(path) = result.output.get("path").and_then(Value::as_str) {
                    activity.files_read.insert(path.to_owned());
                }
            }
            "write" | "edit" => {
                if let Some(path) = result.output.get("path").and_then(Value::as_str) {
                    activity.files_modified.insert(path.to_owned());
                }
            }
            "search" => {
                if let Some(matches) = result.output.get("matches").and_then(Value::as_array) {
                    for path in matches
                        .iter()
                        .filter_map(|entry| entry.get("path").and_then(Value::as_str))
                    {
                        activity.files_read.insert(path.to_owned());
                    }
                }
            }
            _ => {}
        }
    }
    activity
}

fn build_summary(
    dropped_messages: &[Message],
    file_activity: &FileActivity,
    max_tokens: u64,
) -> Option<String> {
    if dropped_messages.is_empty() || max_tokens == 0 {
        return None;
    }

    let mut lines =
        vec!["Earlier conversation compacted to fit the model context window.".to_owned()];
    if !file_activity.files_read.is_empty() {
        lines.push(format!(
            "Files read: {}",
            file_activity
                .files_read
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !file_activity.files_modified.is_empty() {
        lines.push(format!(
            "Files modified: {}",
            file_activity
                .files_modified
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let mut message_lines = dropped_messages
        .iter()
        .rev()
        .take(8)
        .map(summary_line)
        .collect::<Vec<_>>();
    message_lines.reverse();
    lines.extend(message_lines);
    let protected_line_count = lines.len().min(
        1 + usize::from(!file_activity.files_read.is_empty())
            + usize::from(!file_activity.files_modified.is_empty()),
    );

    while !lines.is_empty() {
        let candidate = lines.join("\n");
        if estimate_text_tokens(&candidate) <= max_tokens {
            return Some(candidate);
        }
        if lines.len() > protected_line_count {
            lines.pop();
            continue;
        }
        let approx_bytes = (max_tokens.saturating_mul(4)) as usize;
        let mut truncated = candidate.chars().take(approx_bytes).collect::<String>();
        if truncated.len() < candidate.len() {
            truncated.push_str("...");
        }
        return Some(truncated);
    }

    None
}

fn summary_line(message: &Message) -> String {
    if let Some(call) = message.tool_call() {
        return format!(
            "- Assistant requested tool {} with {}",
            call.tool_name,
            truncate_text(&call.arguments.to_string(), 4, 160).replace('\n', " ")
        );
    }
    if let Some(result) = message.tool_result() {
        return format!(
            "- Tool {} returned {}",
            result.tool_name,
            truncate_text(&result.output.to_string(), 4, 200).replace('\n', " ")
        );
    }

    let rendered = render_message_parts_for_summary(&message.parts);
    format!(
        "- {:?}: {}",
        message.role,
        truncate_text(&rendered, 4, 240).replace('\n', " ")
    )
}

#[must_use]
pub fn estimate_messages_tokens(messages: &[Message]) -> u64 {
    messages.iter().map(estimate_message_tokens).sum()
}

fn estimate_message_tokens(message: &Message) -> u64 {
    message.parts.iter().map(estimate_message_part_tokens).sum()
}

fn estimate_message_part_tokens(part: &MessagePart) -> u64 {
    match part {
        MessagePart::Text { text } => estimate_text_tokens(text),
        MessagePart::ToolCall { call } => {
            estimate_text_tokens(&call.tool_name)
                + estimate_text_tokens(&call.arguments.to_string())
        }
        MessagePart::ToolResult { result } => {
            estimate_text_tokens(&result.tool_name)
                + estimate_text_tokens(&result.output.to_string())
        }
        MessagePart::Refusal {
            text: Some(text), ..
        } => estimate_text_tokens(text),
        MessagePart::Structured { value, .. } => estimate_text_tokens(&value.to_string()),
        MessagePart::Reasoning { .. } | MessagePart::Refusal { text: None, .. } => 0,
    }
}

fn render_message_parts_for_summary(parts: &[MessagePart]) -> String {
    let rendered = parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::Text { text } => Some(text.clone()),
            MessagePart::Refusal {
                text: Some(text), ..
            } => Some(text.clone()),
            MessagePart::Structured { value, .. } => Some(value.to_string()),
            MessagePart::Reasoning { .. }
            | MessagePart::ToolCall { .. }
            | MessagePart::ToolResult { .. }
            | MessagePart::Refusal { text: None, .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    if rendered.is_empty() {
        "[non-text message]".to_owned()
    } else {
        rendered
    }
}

#[must_use]
pub fn estimate_text_tokens(text: &str) -> u64 {
    let chars = text.chars().count() as u64;
    (chars / 4).saturating_add(8)
}

#[cfg(test)]
mod tests {
    use super::{
        ContextAssembler, SystemPromptBuilder, SystemPromptInput, estimate_messages_tokens,
    };
    use bt_core::{
        BranchId, ConnectionId, InstructionDocument, Message, MessagePart, PlanInspection,
        PlanItem, PlanStatus, Role, SessionRecord, SessionStatus, SessionToolMode, ThinkingConfig,
        ThinkingEffort, ToolCall, ToolDisplayGroup, ToolExecutionMode, ToolInterruptBehavior,
        ToolMetadata, ToolResultEnvelope, ToolRiskClass, ToolSpec, default_settings_revision_id,
    };
    use camino::Utf8PathBuf;
    use serde_json::json;

    fn test_session() -> SessionRecord {
        SessionRecord {
            session_id: bt_core::SessionId::new(),
            project_root: Utf8PathBuf::from("/tmp/workspace"),
            connection_id: ConnectionId::new("local"),
            model_id: Some("qwen3:latest".to_owned()),
            tool_mode: SessionToolMode::Extended,
            settings_revision_id: default_settings_revision_id(),
            created_at: time::OffsetDateTime::now_utc(),
            updated_at: time::OffsetDateTime::now_utc(),
            status: SessionStatus::Active,
            display_name: None,
            objective: Some("Summarize the current workspace honestly.".to_owned()),
            parent_session_id: None,
            parent_branch_id: None,
            parent_turn_id: None,
        }
    }

    fn test_tool(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.to_owned(),
            description: format!("{name} description"),
            parameters_schema: json!({"type": "object"}),
            metadata: ToolMetadata {
                risk_class: ToolRiskClass::Safe,
                is_read_only: true,
                is_concurrency_safe: true,
                interrupt_behavior: ToolInterruptBehavior::WaitForCompletion,
                execution_mode: ToolExecutionMode::Immediate,
                should_defer: name == "external_lookup",
                catalogue_tags: Vec::new(),
                display_group: ToolDisplayGroup::Codebase,
            },
        }
    }

    #[test]
    fn estimate_is_non_zero_for_simple_message() {
        let messages = vec![bt_core::Message::text(bt_core::Role::User, "hello world")];
        assert!(estimate_messages_tokens(&messages) > 0);
    }

    #[test]
    fn assembler_defaults_chatgpt_reasoning_models_to_highest_effort() {
        let config = bt_core::BelltowerConfig::from_embedded().expect("config");
        let assembler = ContextAssembler::new(config).expect("assembler");

        let output = assembler.build_request_with_metadata(
            ConnectionId::new("chatgpt"),
            "openai-chatgpt",
            "gpt-5.4-mini",
            None,
            vec![Message::text(Role::User, "hello")],
            Vec::new(),
            None,
        );

        assert_eq!(
            output.request.thinking,
            Some(ThinkingConfig {
                enabled: true,
                effort: Some(ThinkingEffort::XHigh),
                budget_tokens: None,
                include_summaries: true,
            })
        );
    }

    #[test]
    fn assembler_does_not_default_local_openai_compatible_thinking() {
        let config = bt_core::BelltowerConfig::from_embedded().expect("config");
        let assembler = ContextAssembler::new(config).expect("assembler");

        let output = assembler.build_request_with_metadata(
            ConnectionId::new("local"),
            "openai-compatible",
            "gpt-5.4-mini",
            None,
            vec![Message::text(Role::User, "hello")],
            Vec::new(),
            None,
        );

        assert_eq!(output.request.thinking, None);
    }

    #[test]
    fn assembler_preserves_explicit_thinking_config() {
        let config = bt_core::BelltowerConfig::from_embedded().expect("config");
        let assembler = ContextAssembler::new(config).expect("assembler");
        let explicit = ThinkingConfig {
            enabled: true,
            effort: Some(ThinkingEffort::Low),
            budget_tokens: Some(512),
            include_summaries: false,
        };

        let output = assembler.build_request_with_metadata(
            ConnectionId::new("chatgpt"),
            "openai-chatgpt",
            "gpt-5.4-mini",
            None,
            vec![Message::text(Role::User, "hello")],
            Vec::new(),
            Some(explicit.clone()),
        );

        assert_eq!(output.request.thinking, Some(explicit));
    }

    #[test]
    fn assembler_defaults_anthropic_thinking_budget_below_output_reserve() {
        let mut config = bt_core::BelltowerConfig::from_embedded().expect("config");
        config.context.reserve_tokens = 4_096;
        let assembler = ContextAssembler::new(config).expect("assembler");

        let output = assembler.build_request_with_metadata(
            ConnectionId::new("anthropic"),
            "anthropic",
            "claude-sonnet-4-6",
            None,
            vec![Message::text(Role::User, "hello")],
            Vec::new(),
            None,
        );

        assert_eq!(
            output.request.thinking,
            Some(ThinkingConfig {
                enabled: true,
                effort: Some(ThinkingEffort::High),
                budget_tokens: Some(3_072),
                include_summaries: true,
            })
        );
    }

    #[test]
    fn system_prompt_builder_emits_sections_in_expected_order() {
        let session = test_session();
        let plan = PlanInspection {
            session_id: session.session_id,
            branch_id: BranchId::new(),
            items: vec![PlanItem {
                id: "p1".to_owned(),
                content: "Inspect Cargo.toml".to_owned(),
                status: PlanStatus::InProgress,
            }],
            updated_at: time::OffsetDateTime::now_utc(),
        };
        let core = InstructionDocument {
            source: "builtin:core".to_owned(),
            title: "core".to_owned(),
            body: "Core harness rules.".to_owned(),
        };
        let overlay = InstructionDocument {
            source: "builtin:provider".to_owned(),
            title: "provider".to_owned(),
            body: "Use tool calls deliberately.".to_owned(),
        };
        let instruction = InstructionDocument {
            source: "/tmp/project.md".to_owned(),
            title: "Project Rules".to_owned(),
            body: "Prefer Rust idioms.".to_owned(),
        };
        let tools = vec![
            test_tool("search"),
            test_tool("read"),
            test_tool("write"),
            test_tool("web_search"),
            test_tool("web_fetch"),
            test_tool("plan"),
            test_tool("ask"),
            test_tool("inspect"),
        ];

        let prompt = SystemPromptBuilder::build(SystemPromptInput {
            session: &session,
            provider_family: "openai-compatible",
            core_prompt: &core,
            provider_overlay: Some(&overlay),
            instructions: &[instruction],
            plan: Some(&plan),
            tools: &tools,
        });

        let core_index = prompt.find("Core harness rules.").expect("core");
        let objective_index = prompt
            .find("Objective:\nSummarize the current workspace honestly.")
            .expect("objective");
        let context_index = prompt.find("Runtime context:").expect("context");
        let tool_index = prompt.find("Tool guidance:").expect("tools");
        let provider_index = prompt
            .find("Provider notes (openai-compatible):")
            .expect("provider");
        let instructions_index = prompt.find("## Project Rules").expect("instructions");
        let plan_index = prompt.find("Active plan:").expect("plan");

        assert!(core_index < objective_index);
        assert!(objective_index < context_index);
        assert!(context_index < tool_index);
        assert!(tool_index < provider_index);
        assert!(provider_index < instructions_index);
        assert!(instructions_index < plan_index);
    }

    #[test]
    fn system_prompt_builder_renders_runtime_context_and_tool_guidance() {
        let mut session = test_session();
        session.tool_mode = SessionToolMode::Standard;
        session.model_id = None;
        let core = InstructionDocument {
            source: "builtin:core".to_owned(),
            title: "core".to_owned(),
            body: "Core harness rules.".to_owned(),
        };
        let prompt = SystemPromptBuilder::build(SystemPromptInput {
            session: &session,
            provider_family: "openai-compatible",
            core_prompt: &core,
            provider_overlay: None,
            instructions: &[],
            plan: None,
            tools: &[
                test_tool("search"),
                test_tool("read"),
                test_tool("write"),
                test_tool("catalogue"),
            ],
        });

        assert!(prompt.contains("- cwd: /tmp/workspace"));
        assert!(prompt.contains("- connection: local"));
        assert!(prompt.contains("- tool mode: standard"));
        assert!(!prompt.contains("- model:"));
        assert!(prompt.contains("- `search`: discover files or matching content"));
        assert!(prompt.contains("- `read`: inspect a known file"));
        assert!(prompt.contains("- `write`: create or replace a file intentionally"));
        assert!(prompt.contains("- `catalogue`: discover additional external tools"));
        assert!(!prompt.contains("- `web_search`:"));
        assert!(!prompt.contains("- `web_fetch`:"));
    }

    #[test]
    fn system_prompt_builder_renders_detected_validation_surfaces() {
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let project_root =
            Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf()).expect("utf8 path");
        std::fs::write(project_root.join("README.md").as_std_path(), "instructions")
            .expect("write readme");
        std::fs::create_dir_all(project_root.join("tests").as_std_path()).expect("mkdir tests");
        std::fs::write(
            project_root.join("tests/test_outputs.py").as_std_path(),
            "do not copy this test body into the prompt",
        )
        .expect("write test");
        std::fs::create_dir_all(project_root.join("schemas").as_std_path()).expect("mkdir schemas");
        std::fs::create_dir_all(project_root.join("target/tests").as_std_path())
            .expect("mkdir ignored target");
        std::fs::write(
            project_root.join("target/tests/test_leak.py").as_std_path(),
            "ignored generated output",
        )
        .expect("write ignored target test");

        let mut session = test_session();
        session.project_root = project_root;
        let core = InstructionDocument {
            source: "builtin:core".to_owned(),
            title: "core".to_owned(),
            body: "Core harness rules.".to_owned(),
        };

        let prompt = SystemPromptBuilder::build(SystemPromptInput {
            session: &session,
            provider_family: "openai-compatible",
            core_prompt: &core,
            provider_overlay: None,
            instructions: &[],
            plan: None,
            tools: &[],
        });

        assert!(prompt.contains("- validation surfaces detected:"));
        assert!(prompt.contains("  - README.md"));
        assert!(prompt.contains("  - schemas/"));
        assert!(prompt.contains("  - tests/"));
        assert!(prompt.contains("  - tests/test_outputs.py"));
        assert!(!prompt.contains("do not copy this test body into the prompt"));
        assert!(!prompt.contains("target/tests/test_leak.py"));
    }

    #[test]
    fn assembler_compacts_when_over_budget() {
        let mut config = bt_core::BelltowerConfig::from_embedded().expect("config");
        config.context.reserve_tokens = 32_700;
        let assembler = ContextAssembler::new(config).expect("assembler");
        let messages = vec![
            bt_core::Message::text(Role::User, "first task context ".repeat(300)),
            bt_core::Message::text(Role::Assistant, "acknowledged ".repeat(200)),
            bt_core::Message::text(Role::User, "current request"),
        ];

        let output = assembler.build_request_with_metadata(
            ConnectionId::new("local"),
            "openai-compatible",
            "qwen3:latest",
            None,
            messages,
            Vec::new(),
            None,
        );

        let compaction = output.compaction.expect("compaction report");
        assert!(
            compaction
                .summary
                .contains("Earlier conversation compacted")
        );
        assert_eq!(
            compaction.trigger,
            bt_core::ContextCompactionTrigger::TokenBudget
        );
        let summary_message_id = compaction
            .summary_message_id
            .expect("summary message id should be recorded");
        assert!(output.request.messages.iter().any(|message| {
            message.message_id == summary_message_id
                && message.role == Role::System
                && message
                    .text_parts()
                    .any(|text| text.contains("Earlier conversation compacted"))
        }));
        assert!(compaction.first_kept_message_id.is_some());
        assert!(output.request.messages.len() <= 3);
        assert!(
            super::estimate_messages_tokens(&output.request.messages)
                <= assembler
                    .context_window_for("openai-compatible", "qwen3:latest")
                    .saturating_sub(assembler.clone().config.context.reserve_tokens)
        );
    }

    #[test]
    fn compaction_keeps_tool_call_and_result_together() {
        let mut config = bt_core::BelltowerConfig::from_embedded().expect("config");
        config.context.reserve_tokens = 32_700;
        config.context.max_tool_result_lines = 8;
        config.context.max_tool_result_bytes = 160;
        let assembler = ContextAssembler::new(config).expect("assembler");
        let tool_call = Message {
            message_id: bt_core::MessageId::new(),
            role: Role::Assistant,
            parts: vec![MessagePart::ToolCall {
                call: ToolCall {
                    tool_name: "read".to_owned(),
                    call_id: "call-1".to_owned(),
                    arguments: json!({"path": "src/lib.rs"}),
                },
            }],
            created_at: time::OffsetDateTime::now_utc(),
        };
        let tool_result = Message {
            message_id: bt_core::MessageId::new(),
            role: Role::Tool,
            parts: vec![MessagePart::ToolResult {
                result: ToolResultEnvelope {
                    call_id: bt_core::ToolCallId::new("call-1"),
                    tool_name: "read".to_owned(),
                    is_error: false,
                    output: json!({"path": "src/lib.rs", "content": "fn main() {}\n".repeat(400)}),
                    duration_ms: None,
                },
            }],
            created_at: time::OffsetDateTime::now_utc(),
        };
        let messages = vec![
            Message::text(Role::User, "old context ".repeat(400)),
            tool_call.clone(),
            tool_result.clone(),
            Message::text(Role::User, "latest request"),
        ];

        let output = assembler.build_request_with_metadata(
            ConnectionId::new("local"),
            "openai-compatible",
            "qwen3:latest",
            None,
            messages,
            Vec::new(),
            None,
        );

        let tool_call_retained = output.request.messages.iter().any(|message| {
            message
                .tool_call()
                .is_some_and(|call| call.call_id == "call-1")
        });
        let tool_result_retained = output.request.messages.iter().any(|message| {
            message
                .tool_result()
                .is_some_and(|result| result.call_id.to_string() == "call-1")
        });
        assert_eq!(tool_call_retained, tool_result_retained);
    }

    #[test]
    fn compaction_tracks_files_from_dropped_messages() {
        let mut config = bt_core::BelltowerConfig::from_embedded().expect("config");
        config.context.reserve_tokens = 32_650;
        let assembler = ContextAssembler::new(config).expect("assembler");
        let dropped = vec![
            Message {
                message_id: bt_core::MessageId::new(),
                role: Role::Tool,
                parts: vec![MessagePart::ToolResult {
                    result: ToolResultEnvelope {
                        call_id: bt_core::ToolCallId::new("read-1"),
                        tool_name: "read".to_owned(),
                        is_error: false,
                        output: json!({"path": "src/lib.rs", "content": "x".repeat(8000)}),
                        duration_ms: None,
                    },
                }],
                created_at: time::OffsetDateTime::now_utc(),
            },
            Message {
                message_id: bt_core::MessageId::new(),
                role: Role::Tool,
                parts: vec![MessagePart::ToolResult {
                    result: ToolResultEnvelope {
                        call_id: bt_core::ToolCallId::new("edit-1"),
                        tool_name: "edit".to_owned(),
                        is_error: false,
                        output: json!({"path": "src/main.rs", "edited": true, "diff": "y".repeat(4000)}),
                        duration_ms: None,
                    },
                }],
                created_at: time::OffsetDateTime::now_utc(),
            },
            Message::text(Role::User, "latest request"),
        ];

        let output = assembler.build_request_with_metadata(
            ConnectionId::new("local"),
            "openai-compatible",
            "qwen3:latest",
            None,
            dropped,
            Vec::new(),
            None,
        );

        let compaction = output.compaction.expect("compaction report");
        assert!(compaction.files_read.contains(&"src/lib.rs".to_owned()));
        assert!(
            compaction
                .files_modified
                .contains(&"src/main.rs".to_owned())
        );
        assert!(compaction.summary.contains("src/lib.rs"));
        assert!(compaction.summary.contains("src/main.rs"));
    }

    #[test]
    fn forced_compaction_summarizes_even_when_context_fits() {
        let config = bt_core::BelltowerConfig::from_embedded().expect("config");
        let assembler = ContextAssembler::new(config).expect("assembler");
        let messages = vec![
            Message::text(Role::User, "first task context"),
            Message::text(Role::Assistant, "acknowledged"),
            Message::text(Role::User, "latest request"),
        ];

        let output = assembler.build_request_with_options(
            ConnectionId::new("local"),
            "openai-compatible",
            "qwen3:latest",
            None,
            messages,
            Vec::new(),
            None,
            true,
        );

        let compaction = output.compaction.expect("forced compaction report");
        assert!(
            compaction
                .summary
                .contains("Earlier conversation compacted")
        );
        assert!(output.request.messages.len() <= 3);
    }
}
