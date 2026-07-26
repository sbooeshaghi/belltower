#![forbid(unsafe_code)]

use bt_core::{
    BelltowerConfig, CompletionRequest, ConnectionId, ContextCompactionTrigger,
    ContextWindowCatalog, InstructionDocument, Message, MessageId, MessagePart, PlanInspection,
    PlanStatus, Result, Role, SessionRecord, ThinkingConfig, ThinkingEffort, ToolSpec,
};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::fs;

/// Stable prefix marking synthetic compaction summary messages. A previous
/// summary message found in the input history is folded into the next
/// summarization input and never retained alongside the new summary
/// (chain-not-stack composition).
pub const COMPACTION_SUMMARY_MARKER: &str = "[compaction summary]";

/// First line of the deterministic fallback digest. Summary messages written
/// before the marker existed start with this line; recognize them so legacy
/// summaries still fold into the chain.
const LEGACY_SUMMARY_FIRST_LINE: &str =
    "Earlier conversation compacted to fit the model context window.";

/// Fraction of the model window used for the proactive trigger when the
/// configured value is out of range.
const DEFAULT_TRIGGER_FRACTION: f64 = 0.9;

/// Bounds for the token budget reserved for the summary inside the compacted
/// context (an eighth of the available budget, clamped).
const SUMMARY_BUDGET_MIN_TOKENS: u64 = 64;
const SUMMARY_BUDGET_MAX_TOKENS: u64 = 4_096;

/// Estimated prompt overhead reserved when sizing the summarization input.
const SUMMARIZATION_PROMPT_OVERHEAD_TOKENS: u64 = 768;

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

/// Compaction-relevant inputs for one request build. The assembler stays
/// pure: provider-observed usage and the previous summary chain state are
/// computed by the runtime and passed in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContextBuildOptions {
    pub force_compaction: bool,
    /// Provider-observed context size: last real `completion.finished` usage
    /// total plus an estimate of messages appended since. `None` when no
    /// prior completion exists; the whole-history estimate then decides.
    pub observed_context_tokens: Option<u64>,
    /// Summary text of the previous `context.compacted` event on this
    /// branch. Folded into the new summary input, never retained.
    pub previous_summary: Option<String>,
    /// Model-written summary body from the recorded summarization
    /// completion. `None` falls back to the deterministic digest.
    pub summary_override: Option<String>,
}

/// Pure description of the summarization work a triggered compaction needs.
/// The runtime runs the actual completion and passes the result back through
/// [`ContextBuildOptions::summary_override`].
#[derive(Clone, Debug, PartialEq)]
pub struct CompactionPlan {
    pub trigger: ContextCompactionTrigger,
    /// Plaintext transcript of the previous summary plus the messages that
    /// will be dropped, sized to the summarizer model's window. Submit as
    /// the single user message of the summarization completion.
    pub summarization_transcript: String,
    /// Token budget reserved for the summary in the compacted context; use
    /// as the summarization completion's `max_tokens`.
    pub summary_budget_tokens: u64,
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
        options: ContextBuildOptions,
    ) -> ContextBuildOutput {
        let window_tokens = self.context_window_for(provider, model);
        let max_prompt_tokens = window_tokens.saturating_sub(self.config.context.reserve_tokens);
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

        let trigger = compaction_trigger(
            options.force_compaction,
            tokens_before,
            max_prompt_tokens,
            options.observed_context_tokens,
            trigger_tokens(
                window_tokens,
                self.config.context.compaction_trigger_fraction,
            ),
        );
        let (selected_messages, compaction) = if let Some(trigger) = trigger {
            let effective_budget = effective_prompt_budget(
                max_prompt_tokens,
                tokens_before,
                options.observed_context_tokens,
            );
            compact_messages(
                normalized_messages,
                effective_budget.saturating_sub(system_tokens),
                trigger,
                self.config.context.compaction_user_message_budget_tokens,
                options.previous_summary.as_deref(),
                options.summary_override.as_deref(),
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
            ContextBuildOptions::default(),
        )
    }

    /// Decides whether the next build for these inputs will compact and, if
    /// so, describes the summarization work: which content is being dropped
    /// (rendered as a plaintext transcript sized for `summarizer_model`) and
    /// the summary token budget. Pure — runs no completion and records
    /// nothing. Returns `None` when no compaction would occur or when there
    /// is nothing to summarize.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn plan_compaction(
        &self,
        provider: &str,
        model: &str,
        summarizer_model: &str,
        system_prompt: Option<&str>,
        messages: &[Message],
        force_compaction: bool,
        observed_context_tokens: Option<u64>,
        previous_summary: Option<&str>,
    ) -> Option<CompactionPlan> {
        let window_tokens = self.context_window_for(provider, model);
        let max_prompt_tokens = window_tokens.saturating_sub(self.config.context.reserve_tokens);
        let system_tokens = system_prompt.map(estimate_text_tokens).unwrap_or(0);
        let normalized_messages = normalize_messages(
            messages.to_vec(),
            self.config.context.max_tool_result_lines,
            self.config.context.max_tool_result_bytes,
        );
        let tokens_before =
            estimate_messages_tokens(&normalized_messages).saturating_add(system_tokens);
        let trigger = compaction_trigger(
            force_compaction,
            tokens_before,
            max_prompt_tokens,
            observed_context_tokens,
            trigger_tokens(
                window_tokens,
                self.config.context.compaction_trigger_fraction,
            ),
        )?;
        let effective_budget =
            effective_prompt_budget(max_prompt_tokens, tokens_before, observed_context_tokens);
        let selection = resolve_retention_selection(
            normalized_messages,
            effective_budget.saturating_sub(system_tokens),
            self.config.context.compaction_user_message_budget_tokens,
            trigger == ContextCompactionTrigger::Forced,
            previous_summary,
        );
        if selection.dropped.is_empty() && selection.folded_summaries.is_empty() {
            return None;
        }

        let summarizer_window = self.context_window_for(provider, summarizer_model);
        let input_budget = summarizer_window
            .saturating_sub(self.config.context.reserve_tokens)
            .saturating_sub(selection.summary_budget)
            .saturating_sub(SUMMARIZATION_PROMPT_OVERHEAD_TOKENS)
            .max(512);
        Some(CompactionPlan {
            trigger,
            summarization_transcript: render_summarization_transcript(
                &selection.folded_summaries,
                &selection.dropped,
                input_budget,
            ),
            summary_budget_tokens: selection.summary_budget,
        })
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

/// Trigger threshold in tokens: `fraction` of the full model window,
/// falling back to the default fraction when the configured value is out of
/// the valid `(0, 1]` range.
fn trigger_tokens(window_tokens: u64, fraction: f64) -> u64 {
    let fraction = if fraction > 0.0 && fraction <= 1.0 {
        fraction
    } else {
        DEFAULT_TRIGGER_FRACTION
    };
    (window_tokens as f64 * fraction).floor() as u64
}

/// Decides whether compaction fires. Provider-observed usage beats the local
/// estimate for the proactive check; the whole-history estimate remains the
/// hard fit guarantee.
fn compaction_trigger(
    force_compaction: bool,
    estimated_tokens: u64,
    max_prompt_tokens: u64,
    observed_context_tokens: Option<u64>,
    trigger_tokens: u64,
) -> Option<ContextCompactionTrigger> {
    if force_compaction {
        return Some(ContextCompactionTrigger::Forced);
    }
    if estimated_tokens > max_prompt_tokens {
        return Some(ContextCompactionTrigger::TokenBudget);
    }
    if observed_context_tokens.is_some_and(|observed| observed > trigger_tokens) {
        return Some(ContextCompactionTrigger::TokenBudget);
    }
    None
}

/// Retention budget in estimated-token space. When the provider observed
/// more tokens than the local estimate for the same content, the estimator
/// undercounts; scale the budget down proportionally so retention fits the
/// real window. An observation below the estimate never grows the budget.
fn effective_prompt_budget(
    max_prompt_tokens: u64,
    estimated_tokens: u64,
    observed_context_tokens: Option<u64>,
) -> u64 {
    match observed_context_tokens {
        Some(observed) if observed > estimated_tokens && observed > 0 => {
            ((u128::from(max_prompt_tokens) * u128::from(estimated_tokens.max(1)))
                / u128::from(observed)) as u64
        }
        _ => max_prompt_tokens,
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
    bt_core::model_capability::openai_default_reasoning_effort(model)
}

fn supports_anthropic_thinking(model: &str) -> bool {
    bt_core::model_capability::anthropic_supports_thinking(model)
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

/// One compaction retention decision: what stays verbatim, what gets
/// summarized, and which previous summaries fold into the chain.
#[derive(Clone, Debug)]
struct RetentionSelection {
    pinned_prefix: Vec<Message>,
    /// Older real user messages kept verbatim (chronological order),
    /// selected newest-first within the user budget.
    retained_users: Vec<Message>,
    /// Recent tail kept verbatim, all roles, cut only at message-unit
    /// boundaries (chronological order).
    tail: Vec<Message>,
    /// Messages leaving the selected context; the summarization input
    /// (chronological order, excludes retained user messages).
    dropped: Vec<Message>,
    /// Bodies of previous compaction summaries (event chain state plus any
    /// summary messages found in the history), oldest first. Folded into
    /// the summarization input and never retained.
    folded_summaries: Vec<String>,
    /// Token budget reserved for the summary message.
    summary_budget: u64,
    messages_before: u32,
}

/// Selects retention for one compaction: [pinned system prefix] + [older
/// user messages within budget] + [summary] + [assistant-boundary tail],
/// then shrinks deterministically until the estimated total (with the
/// summary budget as a placeholder) fits `budget_tokens`.
fn resolve_retention_selection(
    messages: Vec<Message>,
    budget_tokens: u64,
    user_budget_config: u64,
    force_compaction: bool,
    previous_summary: Option<&str>,
) -> RetentionSelection {
    let messages_before = messages.len() as u32;
    let (mut folded_summaries, messages) = extract_summary_messages(messages);
    if let Some(previous) = previous_summary {
        let body = normalize_summary_body(previous);
        if !body.is_empty() && !folded_summaries.iter().any(|existing| *existing == body) {
            folded_summaries.insert(0, body);
        }
    }

    let (pinned_prefix, remainder) = split_system_prefix(messages);
    let prefix_tokens = estimate_messages_tokens(&pinned_prefix);
    let units = build_message_units(remainder);

    let available = budget_tokens.saturating_sub(prefix_tokens);
    let summary_budget =
        (available / 8).clamp(SUMMARY_BUDGET_MIN_TOKENS, SUMMARY_BUDGET_MAX_TOKENS);
    let content_budget = available.saturating_sub(summary_budget);
    let user_budget = user_budget_config.min(content_budget / 2);
    let tail_budget = content_budget.saturating_sub(user_budget);

    // Tail: walk backward from the newest unit, cutting only at unit
    // boundaries; the last unit is always kept.
    let mut tail_units: Vec<MessageUnit> = Vec::new();
    let mut tail_tokens = 0u64;
    let mut cut_index = units.len();
    for (index, unit) in units.iter().enumerate().rev() {
        let unit_tokens = estimate_messages_tokens(&unit.messages);
        if tail_units.is_empty() || tail_tokens.saturating_add(unit_tokens) <= tail_budget {
            tail_units.insert(0, unit.clone());
            tail_tokens = tail_tokens.saturating_add(unit_tokens);
            cut_index = index;
        } else {
            break;
        }
    }
    let mut dropped_units: Vec<MessageUnit> = units[..cut_index].to_vec();
    // The retained tail must never open on an orphan tool-result message.
    while tail_units.len() > 1
        && tail_units
            .first()
            .and_then(|unit| unit.messages.first())
            .is_some_and(|message| message.role == Role::Tool)
    {
        dropped_units.push(tail_units.remove(0));
    }

    // Older real user messages: newest-first selection within the user
    // budget, retained verbatim in chronological order.
    let dropped_messages = flatten_units(&dropped_units);
    let mut retained_user_ids = BTreeSet::new();
    let mut user_tokens = 0u64;
    for message in dropped_messages.iter().rev() {
        if message.role != Role::User {
            continue;
        }
        if message.text_parts().next().is_none() {
            continue;
        }
        let message_tokens = estimate_message_tokens(message);
        if user_tokens.saturating_add(message_tokens) > user_budget {
            continue;
        }
        user_tokens = user_tokens.saturating_add(message_tokens);
        retained_user_ids.insert(message.message_id);
    }
    let mut retained_users: Vec<Message> = dropped_messages
        .iter()
        .filter(|message| retained_user_ids.contains(&message.message_id))
        .cloned()
        .collect();
    let mut dropped: Vec<Message> = dropped_messages
        .into_iter()
        .filter(|message| !retained_user_ids.contains(&message.message_id))
        .collect();

    // Forced compaction must summarize something even when everything
    // fits: age the oldest tail unit(s) into the dropped set. Forced drops
    // are not re-offered verbatim user retention.
    while force_compaction && dropped.is_empty() && tail_units.len() > 1 {
        let unit = tail_units.remove(0);
        dropped.extend(unit.messages);
    }

    // Deterministic shrink until the placeholder-estimated total fits.
    loop {
        let tail_messages = flatten_units(&tail_units);
        let total = prefix_tokens
            .saturating_add(estimate_messages_tokens(&retained_users))
            .saturating_add(summary_budget)
            .saturating_add(estimate_messages_tokens(&tail_messages));
        if total <= budget_tokens {
            break;
        }
        if !retained_users.is_empty() {
            let removed = retained_users.remove(0);
            dropped.push(removed);
            dropped.sort_by_key(|message| message.created_at);
            continue;
        }
        if tail_units.len() > 1 {
            let unit = tail_units.remove(0);
            dropped.extend(unit.messages);
            continue;
        }
        break;
    }

    RetentionSelection {
        pinned_prefix,
        retained_users,
        tail: flatten_units(&tail_units),
        dropped,
        folded_summaries,
        summary_budget,
        messages_before,
    }
}

fn compact_messages(
    messages: Vec<Message>,
    budget_tokens: u64,
    trigger: ContextCompactionTrigger,
    user_budget_config: u64,
    previous_summary: Option<&str>,
    summary_override: Option<&str>,
) -> (Vec<Message>, Option<ContextCompaction>) {
    let selection = resolve_retention_selection(
        messages,
        budget_tokens,
        user_budget_config,
        trigger == ContextCompactionTrigger::Forced,
        previous_summary,
    );
    let file_activity = collect_file_activity(&selection.dropped);

    let non_summary_tokens = estimate_messages_tokens(&selection.pinned_prefix)
        .saturating_add(estimate_messages_tokens(&selection.retained_users))
        .saturating_add(estimate_messages_tokens(&selection.tail));
    let summary_token_room = budget_tokens
        .saturating_sub(non_summary_tokens)
        .clamp(16, selection.summary_budget.max(16));
    let summary_body = match summary_override {
        Some(text) if !text.trim().is_empty() => Some(truncate_to_token_estimate(
            text.trim(),
            summary_token_room.max(selection.summary_budget),
        )),
        _ => compose_fallback_summary_body(
            &selection.dropped,
            &selection.folded_summaries,
            &file_activity,
            summary_token_room,
        ),
    };

    let Some(summary_body) = summary_body else {
        // Nothing dropped and no chain to carry: leave the context as-is.
        let mut original = selection.pinned_prefix;
        original.extend(selection.retained_users);
        original.extend(selection.dropped);
        original.extend(selection.tail);
        return (original, None);
    };

    let summary_text = format!("{COMPACTION_SUMMARY_MARKER}\n{summary_body}");
    let summary_message = Message::text(Role::System, summary_text.clone());
    let summary_message_id = summary_message.message_id;
    let first_kept_message_id = selection
        .tail
        .first()
        .or_else(|| selection.retained_users.first())
        .map(|message| message.message_id);

    let mut selected_messages = selection.pinned_prefix;
    selected_messages.extend(selection.retained_users);
    selected_messages.push(summary_message);
    selected_messages.extend(selection.tail);

    let compaction = ContextCompaction {
        summary: summary_text,
        trigger,
        reason: Some(match trigger {
            ContextCompactionTrigger::Forced => "forced".to_owned(),
            ContextCompactionTrigger::TokenBudget => "token_budget".to_owned(),
        }),
        summary_message_id: Some(summary_message_id),
        first_kept_message_id,
        messages_before: selection.messages_before,
        messages_after: 0,
        tokens_before: 0,
        tokens_after: 0,
        files_read: file_activity.files_read.into_iter().collect(),
        files_modified: file_activity.files_modified.into_iter().collect(),
    };
    (selected_messages, Some(compaction))
}

/// Removes synthetic compaction summary messages from the history and
/// returns their bodies (oldest first) for chain folding.
fn extract_summary_messages(messages: Vec<Message>) -> (Vec<String>, Vec<Message>) {
    let mut folded = Vec::new();
    let mut remaining = Vec::with_capacity(messages.len());
    for message in messages {
        if let Some(body) = summary_message_body(&message) {
            if !folded.contains(&body) {
                folded.push(body);
            }
        } else {
            remaining.push(message);
        }
    }
    (folded, remaining)
}

/// Recognizes a synthetic compaction summary message (marker prefix, or the
/// legacy digest first line) and returns its body text.
fn summary_message_body(message: &Message) -> Option<String> {
    if message.role != Role::System {
        return None;
    }
    let text = message.text_parts().collect::<Vec<_>>().join("\n");
    let trimmed = text.trim_start();
    if trimmed.starts_with(COMPACTION_SUMMARY_MARKER)
        || trimmed.starts_with(LEGACY_SUMMARY_FIRST_LINE)
    {
        let body = normalize_summary_body(trimmed);
        (!body.is_empty()).then_some(body)
    } else {
        None
    }
}

/// Strips the summary marker prefix (when present) and surrounding
/// whitespace, leaving the summary body.
fn normalize_summary_body(text: &str) -> String {
    let trimmed = text.trim();
    trimmed
        .strip_prefix(COMPACTION_SUMMARY_MARKER)
        .unwrap_or(trimmed)
        .trim()
        .to_owned()
}

/// Deterministic digest fallback: the rule-based digest of the dropped
/// messages first (legacy shape), then previous summaries carried forward so
/// the chain never loses its accumulated state.
fn compose_fallback_summary_body(
    dropped: &[Message],
    folded_summaries: &[String],
    file_activity: &FileActivity,
    budget_tokens: u64,
) -> Option<String> {
    let mut sections = Vec::new();
    if !folded_summaries.is_empty() {
        let folded_budget = (budget_tokens / 2).max(SUMMARY_BUDGET_MIN_TOKENS);
        sections.push(truncate_to_token_estimate(
            &format!("Previous summary:\n{}", folded_summaries.join("\n\n")),
            folded_budget,
        ));
    }
    let folded_tokens = sections
        .iter()
        .map(|section| estimate_text_tokens(section))
        .sum::<u64>();
    if let Some(digest) = build_summary(
        dropped,
        file_activity,
        budget_tokens.saturating_sub(folded_tokens).max(64),
    ) {
        sections.insert(0, digest);
    }
    (!sections.is_empty()).then(|| sections.join("\n"))
}

fn truncate_to_token_estimate(text: &str, budget_tokens: u64) -> String {
    if estimate_text_tokens(text) <= budget_tokens {
        return text.to_owned();
    }
    let approx_chars = (budget_tokens.saturating_mul(4)) as usize;
    let mut truncated = text.chars().take(approx_chars).collect::<String>();
    truncated.push_str("...");
    truncated
}

/// Renders the summarization input: previous summaries first, then the
/// dropped messages as a plaintext transcript. When the input budget binds,
/// the oldest dropped messages are elided (they are covered by the previous
/// summary chain) and the elision is stated.
fn render_summarization_transcript(
    folded_summaries: &[String],
    dropped: &[Message],
    input_budget_tokens: u64,
) -> String {
    let mut header = String::new();
    if !folded_summaries.is_empty() {
        header.push_str(
            "Previous compaction summary (fold its still-relevant content into the new summary):\n",
        );
        header.push_str(&folded_summaries.join("\n\n"));
        header.push_str("\n\n");
    }

    let lines: Vec<String> = dropped.iter().map(render_message_for_transcript).collect();
    let line_tokens: Vec<u64> = lines
        .iter()
        .map(|line| estimate_text_tokens(line))
        .collect();
    let header_tokens = estimate_text_tokens(&header).saturating_add(64);
    let body_budget = input_budget_tokens.saturating_sub(header_tokens);
    let mut start = lines.len();
    let mut used = 0u64;
    while start > 0 {
        let next = used.saturating_add(line_tokens[start - 1]);
        if next > body_budget && start < lines.len() {
            break;
        }
        used = next;
        start -= 1;
    }

    let mut transcript = header;
    if lines.is_empty() {
        transcript.push_str("No additional messages are being dropped in this compaction.\n");
        return transcript;
    }
    if start > 0 {
        transcript.push_str(&format!(
            "[{start} older dropped messages omitted from this excerpt; they are covered by the previous summary and remain in session history]\n"
        ));
    }
    transcript.push_str("Messages being dropped from the model-visible context:\n");
    transcript.push_str(&lines[start..].join("\n"));
    transcript
}

fn render_message_for_transcript(message: &Message) -> String {
    let role = match message.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    };
    let rendered = message
        .parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::Text { text } => Some(text.clone()),
            MessagePart::ToolCall { call } => Some(format!(
                "[tool_call {} {}]",
                call.tool_name,
                truncate_chars(&call.arguments.to_string(), 400)
            )),
            MessagePart::ToolResult { result } => Some(format!(
                "[tool_result {}{} {}]",
                result.tool_name,
                if result.is_error { " error" } else { "" },
                truncate_chars(&result.output.to_string(), 800)
            )),
            MessagePart::Refusal {
                text: Some(text), ..
            } => Some(format!("[refusal] {text}")),
            MessagePart::Structured { value, .. } => Some(format!(
                "[structured] {}",
                truncate_chars(&value.to_string(), 400)
            )),
            MessagePart::Reasoning { .. } | MessagePart::Refusal { text: None, .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let rendered = if rendered.is_empty() {
        "[non-text message]".to_owned()
    } else {
        rendered
    };
    format!("[{role}] {rendered}")
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut truncated = text.chars().take(max_chars).collect::<String>();
    if truncated.chars().count() < text.chars().count() {
        truncated.push_str("...");
    }
    truncated.replace('\n', " ")
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

/// Groups messages into indivisible retention units. A unit opens at any
/// message and, while it has tool calls awaiting results, absorbs following
/// messages (matching results, further same-step tool calls, and anything
/// interposed between a call and its result). Cuts happen only at unit
/// boundaries, so a tool_use and its tool_result — and an approval-paused
/// assistant call and its later result — are never split.
fn build_message_units(messages: Vec<Message>) -> Vec<MessageUnit> {
    let mut units = Vec::new();
    let mut index = 0usize;
    while index < messages.len() {
        let mut pending = message_tool_call_ids(&messages[index])
            .into_iter()
            .collect::<BTreeSet<_>>();
        let mut unit_messages = vec![messages[index].clone()];
        index += 1;
        while !pending.is_empty() && index < messages.len() {
            let next = &messages[index];
            for call_id in message_tool_result_ids(next) {
                pending.remove(&call_id);
            }
            for call_id in message_tool_call_ids(next) {
                pending.insert(call_id);
            }
            unit_messages.push(next.clone());
            index += 1;
        }
        units.push(MessageUnit {
            messages: unit_messages,
        });
    }
    units
}

fn message_tool_call_ids(message: &Message) -> Vec<String> {
    message
        .parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::ToolCall { call } => Some(call.call_id.clone()),
            _ => None,
        })
        .collect()
}

fn message_tool_result_ids(message: &Message) -> Vec<String> {
    message
        .parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::ToolResult { result } => Some(result.call_id.to_string()),
            _ => None,
        })
        .collect()
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
    fn assembler_defaults_chatgpt_reasoning_models_to_high_effort() {
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
                effort: Some(ThinkingEffort::High),
                budget_tokens: None,
                include_summaries: true,
            })
        );
    }

    #[test]
    fn assembler_defaults_codex_max_to_xhigh_effort() {
        let config = bt_core::BelltowerConfig::from_embedded().expect("config");
        let assembler = ContextAssembler::new(config).expect("assembler");

        let output = assembler.build_request_with_metadata(
            ConnectionId::new("chatgpt"),
            "openai-chatgpt",
            "gpt-5.1-codex-max",
            None,
            vec![Message::text(Role::User, "hello")],
            Vec::new(),
            None,
        );

        assert_eq!(
            output.request.thinking.and_then(|t| t.effort),
            Some(ThinkingEffort::XHigh)
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
            super::ContextBuildOptions {
                force_compaction: true,
                ..Default::default()
            },
        );

        let compaction = output.compaction.expect("forced compaction report");
        assert!(
            compaction
                .summary
                .contains("Earlier conversation compacted")
        );
        assert!(output.request.messages.len() <= 3);
    }

    fn compaction_options(
        observed_context_tokens: Option<u64>,
        previous_summary: Option<&str>,
    ) -> super::ContextBuildOptions {
        super::ContextBuildOptions {
            force_compaction: false,
            observed_context_tokens,
            previous_summary: previous_summary.map(ToOwned::to_owned),
            summary_override: None,
        }
    }

    fn message_text(message: &Message) -> String {
        message.text_parts().collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn compaction_orders_prefix_users_summary_tail_with_verbatim_users() {
        let config = bt_core::BelltowerConfig::from_embedded().expect("config");
        let assembler = ContextAssembler::new(config).expect("assembler");
        let messages = vec![
            Message::text(Role::System, "pinned system preamble"),
            Message::text(Role::User, "old user question alpha"),
            Message::text(Role::Assistant, "assistant chatter ".repeat(4_000)),
            Message::text(Role::User, "old user question beta"),
            Message::text(Role::Assistant, "more assistant chatter ".repeat(4_000)),
            Message::text(Role::User, "latest request"),
        ];
        let latest_id = messages.last().expect("latest").message_id;

        let output = assembler.build_request_with_metadata(
            ConnectionId::new("local"),
            "openai-compatible",
            "qwen3:latest",
            None,
            messages,
            Vec::new(),
            None,
        );

        let compaction = output.compaction.expect("compaction fires over budget");
        let rendered: Vec<(Role, String)> = output
            .request
            .messages
            .iter()
            .map(|message| (message.role.clone(), message_text(message)))
            .collect();
        assert_eq!(rendered[0].0, Role::System);
        assert_eq!(rendered[0].1, "pinned system preamble");
        assert_eq!(
            rendered[1],
            (Role::User, "old user question alpha".to_owned())
        );
        assert_eq!(
            rendered[2],
            (Role::User, "old user question beta".to_owned())
        );
        assert_eq!(rendered[3].0, Role::System);
        assert!(rendered[3].1.starts_with(super::COMPACTION_SUMMARY_MARKER));
        assert_eq!(rendered[4], (Role::User, "latest request".to_owned()));
        assert_eq!(
            rendered.len(),
            5,
            "assistant chatter must be summarized away"
        );
        assert_eq!(compaction.first_kept_message_id, Some(latest_id));
        assert_eq!(
            compaction.summary_message_id,
            Some(output.request.messages[3].message_id)
        );
    }

    #[test]
    fn compaction_never_splits_tool_call_from_delayed_result() {
        let config = bt_core::BelltowerConfig::from_embedded().expect("config");
        let assembler = ContextAssembler::new(config).expect("assembler");
        let call_message = Message::from_part(
            Role::Assistant,
            MessagePart::ToolCall {
                call: ToolCall {
                    tool_name: "shell".to_owned(),
                    call_id: "call-delayed".to_owned(),
                    arguments: json!({"command": "cargo test"}),
                },
            },
        );
        // The approval pause interposes a message between the call and its
        // result: the unit must absorb it so the pair is never split.
        let interposed = Message::text(Role::User, "steer note during approval");
        let result_message = Message::from_part(
            Role::Tool,
            MessagePart::ToolResult {
                result: ToolResultEnvelope {
                    call_id: bt_core::ToolCallId::new("call-delayed"),
                    tool_name: "shell".to_owned(),
                    is_error: false,
                    output: json!({"stdout": "x".repeat(200_000)}),
                    duration_ms: None,
                },
            },
        );
        let messages = vec![
            Message::text(Role::User, "old context ".repeat(4_000)),
            call_message,
            interposed,
            result_message,
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

        assert!(output.compaction.is_some());
        let call_retained = output.request.messages.iter().any(|message| {
            message
                .tool_call()
                .is_some_and(|call| call.call_id == "call-delayed")
        });
        let result_retained = output.request.messages.iter().any(|message| {
            message
                .tool_result()
                .is_some_and(|result| result.call_id.to_string() == "call-delayed")
        });
        assert_eq!(
            call_retained, result_retained,
            "tool_use and its delayed tool_result must be kept or dropped together"
        );
        for message in &output.request.messages {
            assert!(
                message.tool_result().is_none()
                    || output.request.messages.iter().any(|candidate| {
                        candidate.tool_call().is_some_and(|call| {
                            Some(call.call_id.clone())
                                == message
                                    .tool_result()
                                    .map(|result| result.call_id.to_string())
                        })
                    }),
                "the retained tail must never open on an orphan tool result"
            );
        }
    }

    #[test]
    fn compaction_user_budget_keeps_newest_users_within_budget() {
        let mut config = bt_core::BelltowerConfig::from_embedded().expect("config");
        config.context.compaction_user_message_budget_tokens = 30;
        let assembler = ContextAssembler::new(config).expect("assembler");
        let old_long_user = "much older long user question ".repeat(20);
        let messages = vec![
            Message::text(Role::User, old_long_user.clone()),
            Message::text(Role::Assistant, "assistant chatter ".repeat(4_000)),
            Message::text(Role::User, "newer short ask"),
            Message::text(Role::Assistant, "more chatter ".repeat(4_000)),
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

        assert!(output.compaction.is_some());
        let texts: Vec<String> = output.request.messages.iter().map(message_text).collect();
        assert!(
            texts.iter().any(|text| text == "newer short ask"),
            "the newest user message within budget is retained verbatim"
        );
        assert!(
            !texts.iter().any(|text| *text == old_long_user),
            "the older user message over budget is summarized, not retained"
        );
    }

    #[test]
    fn compaction_folds_previous_summary_and_never_retains_it() {
        let config = bt_core::BelltowerConfig::from_embedded().expect("config");
        let assembler = ContextAssembler::new(config).expect("assembler");
        let previous_summary_message = Message::text(
            Role::System,
            format!(
                "{}\nkey earlier facts: path /tmp/alpha.rs decided",
                super::COMPACTION_SUMMARY_MARKER
            ),
        );
        let previous_summary_id = previous_summary_message.message_id;
        let messages = vec![
            previous_summary_message,
            Message::text(Role::User, "old question ".repeat(4_000)),
            Message::text(Role::Assistant, "old answer ".repeat(4_000)),
            Message::text(Role::User, "latest request"),
        ];

        let output = assembler.build_request_with_options(
            ConnectionId::new("local"),
            "openai-compatible",
            "qwen3:latest",
            None,
            messages.clone(),
            Vec::new(),
            None,
            compaction_options(None, Some("event-recorded earlier summary body")),
        );

        let compaction = output.compaction.expect("compaction fires");
        let summary_messages: Vec<&Message> = output
            .request
            .messages
            .iter()
            .filter(|message| {
                message.role == Role::System
                    && message_text(message).starts_with(super::COMPACTION_SUMMARY_MARKER)
            })
            .collect();
        assert_eq!(
            summary_messages.len(),
            1,
            "chain-not-stack: exactly one summary message after compaction"
        );
        assert_ne!(
            summary_messages[0].message_id, previous_summary_id,
            "the previous summary message is never retained"
        );
        assert!(
            compaction.summary.contains("key earlier facts"),
            "previous in-history summary folds into the new summary"
        );
        assert!(
            compaction
                .summary
                .contains("event-recorded earlier summary body"),
            "previous event summary folds into the new summary"
        );

        let plan = assembler
            .plan_compaction(
                "openai-compatible",
                "qwen3:latest",
                "qwen3:latest",
                None,
                &messages,
                false,
                None,
                Some("event-recorded earlier summary body"),
            )
            .expect("plan for over-budget history");
        assert!(
            plan.summarization_transcript.contains("key earlier facts"),
            "summarizer input includes previous in-history summary"
        );
        assert!(
            plan.summarization_transcript
                .contains("event-recorded earlier summary body"),
            "summarizer input includes previous event summary"
        );
    }

    #[test]
    fn observed_tokens_trigger_compaction_when_estimate_fits() {
        let config = bt_core::BelltowerConfig::from_embedded().expect("config");
        let assembler = ContextAssembler::new(config).expect("assembler");
        let messages = vec![
            Message::text(Role::User, "first task context"),
            Message::text(Role::Assistant, "acknowledged"),
            Message::text(Role::User, "latest request"),
        ];

        // qwen3 window 32768; default fraction 0.9 -> threshold 29491.
        let over = assembler.build_request_with_options(
            ConnectionId::new("local"),
            "openai-compatible",
            "qwen3:latest",
            None,
            messages.clone(),
            Vec::new(),
            None,
            compaction_options(Some(30_000), None),
        );
        assert!(
            over.compaction.is_some(),
            "provider-observed usage beats the local estimate"
        );
        assert_eq!(
            over.compaction.expect("compaction").trigger,
            bt_core::ContextCompactionTrigger::TokenBudget
        );

        let under = assembler.build_request_with_options(
            ConnectionId::new("local"),
            "openai-compatible",
            "qwen3:latest",
            None,
            messages.clone(),
            Vec::new(),
            None,
            compaction_options(Some(20_000), None),
        );
        assert!(under.compaction.is_none());
    }

    #[test]
    fn compaction_trigger_fraction_config_is_respected() {
        let mut config = bt_core::BelltowerConfig::from_embedded().expect("config");
        config.context.compaction_trigger_fraction = 0.95;
        let assembler = ContextAssembler::new(config).expect("assembler");
        let messages = vec![
            Message::text(Role::User, "first task context"),
            Message::text(Role::Assistant, "acknowledged"),
            Message::text(Role::User, "latest request"),
        ];

        // threshold at 0.95 * 32768 = 31129; 30_000 stays under it.
        let output = assembler.build_request_with_options(
            ConnectionId::new("local"),
            "openai-compatible",
            "qwen3:latest",
            None,
            messages.clone(),
            Vec::new(),
            None,
            compaction_options(Some(30_000), None),
        );
        assert!(output.compaction.is_none());

        let output = assembler.build_request_with_options(
            ConnectionId::new("local"),
            "openai-compatible",
            "qwen3:latest",
            None,
            messages,
            Vec::new(),
            None,
            compaction_options(Some(31_500), None),
        );
        assert!(output.compaction.is_some());
    }

    #[test]
    fn summary_override_is_used_verbatim_with_marker() {
        let config = bt_core::BelltowerConfig::from_embedded().expect("config");
        let assembler = ContextAssembler::new(config).expect("assembler");
        let messages = vec![
            Message::text(Role::User, "old question ".repeat(4_000)),
            Message::text(Role::Assistant, "old answer ".repeat(4_000)),
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
            super::ContextBuildOptions {
                force_compaction: false,
                observed_context_tokens: None,
                previous_summary: None,
                summary_override: Some("model-written checkpoint body".to_owned()),
            },
        );

        let compaction = output.compaction.expect("compaction fires");
        assert!(
            compaction
                .summary
                .starts_with(super::COMPACTION_SUMMARY_MARKER)
        );
        assert!(compaction.summary.contains("model-written checkpoint body"));
        assert!(
            !compaction
                .summary
                .contains("Earlier conversation compacted"),
            "the model summary replaces the deterministic digest"
        );
    }

    #[test]
    fn plan_compaction_is_none_when_context_fits() {
        let config = bt_core::BelltowerConfig::from_embedded().expect("config");
        let assembler = ContextAssembler::new(config).expect("assembler");
        let messages = vec![
            Message::text(Role::User, "small question"),
            Message::text(Role::Assistant, "small answer"),
        ];
        assert!(
            assembler
                .plan_compaction(
                    "openai-compatible",
                    "qwen3:latest",
                    "qwen3:latest",
                    None,
                    &messages,
                    false,
                    None,
                    None,
                )
                .is_none()
        );
    }
}
