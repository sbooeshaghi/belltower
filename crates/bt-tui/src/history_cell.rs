use super::*;
use crate::message_render::render_wrapped_prefixed_entry;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Paragraph, Wrap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) type SharedHistoryCell = Arc<dyn HistoryCell>;

pub(super) trait HistoryCell: std::fmt::Debug + Send + Sync {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>>;

    fn desired_height(&self, width: u16) -> u16 {
        Paragraph::new(Text::from(self.display_lines(width)))
            .wrap(Wrap { trim: false })
            .line_count(width)
            .try_into()
            .unwrap_or(0)
    }

    fn is_stream_continuation(&self) -> bool {
        false
    }

    fn is_assistant_stream_cell(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug)]
pub(super) struct PlainHistoryCell {
    lines: Vec<Line<'static>>,
}

impl PlainHistoryCell {
    pub(super) fn new(lines: Vec<Line<'static>>) -> Self {
        Self { lines }
    }
}

impl HistoryCell for PlainHistoryCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        self.lines.clone()
    }
}

#[derive(Clone, Debug)]
pub(super) struct UserHistoryCell {
    lines: Vec<String>,
}

impl UserHistoryCell {
    pub(super) fn new(lines: Vec<String>) -> Self {
        Self { lines }
    }
}

fn composer_background_line(content: &str, width: usize) -> Line<'static> {
    let padding = width.saturating_sub(crate::custom_terminal::display_width(content));
    let style = user_surface_style();
    let mut spans = Vec::new();
    if let Some(rest) = content.strip_prefix(COMPOSER_PREFIX) {
        spans.push(Span::styled(
            COMPOSER_PREFIX.to_owned(),
            style.add_modifier(Modifier::BOLD | Modifier::DIM),
        ));
        if !rest.is_empty() {
            spans.push(Span::styled(rest.to_owned(), style));
        }
    } else if !content.is_empty() {
        spans.push(Span::styled(content.to_owned(), style));
    }
    if padding > 0 || spans.is_empty() {
        spans.push(Span::styled(" ".repeat(padding), style));
    }
    Line::from(spans)
}

impl HistoryCell for UserHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let width = usize::from(width.max(1));
        let mut lines = Vec::new();
        for _ in 0..COMPOSER_TOP_PADDING {
            lines.push(composer_background_line("", width));
        }
        lines.extend(
            self.lines
                .iter()
                .map(|line| composer_background_line(line, width)),
        );
        for _ in 0..COMPOSER_BOTTOM_PADDING {
            lines.push(composer_background_line("", width));
        }
        lines
    }
}

#[derive(Clone, Debug)]
pub(super) struct AskHistoryCell {
    question: Option<String>,
    answer: Option<String>,
}

impl AskHistoryCell {
    pub(super) fn new(question: Option<String>, answer: Option<String>) -> Self {
        Self { question, answer }
    }
}

impl HistoryCell for AskHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        crate::message_render::render_ask_history_entry(
            self.question.as_deref(),
            self.answer.as_deref(),
            width,
        )
        .lines()
        .map(|line| Line::from(line.to_owned()))
        .collect()
    }
}

#[derive(Clone, Debug)]
pub(super) struct FinalMessageSeparatorCell {
    elapsed_seconds: Option<u64>,
}

impl FinalMessageSeparatorCell {
    pub(super) fn new(elapsed_seconds: Option<u64>) -> Self {
        Self { elapsed_seconds }
    }
}

fn format_elapsed_compact(seconds: u64) -> String {
    if seconds >= 3600 {
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        if minutes == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h {minutes}m")
        }
    } else if seconds >= 60 {
        let minutes = seconds / 60;
        let seconds = seconds % 60;
        if seconds == 0 {
            format!("{minutes}m")
        } else {
            format!("{minutes}m {seconds}s")
        }
    } else {
        format!("{seconds}s")
    }
}

impl HistoryCell for FinalMessageSeparatorCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let width = usize::from(width.max(1));
        let mut label_parts = Vec::new();
        if let Some(elapsed_seconds) = self.elapsed_seconds.filter(|seconds| *seconds > 60) {
            label_parts.push(format!(
                "Worked for {}",
                format_elapsed_compact(elapsed_seconds)
            ));
        }

        let line = if label_parts.is_empty() {
            "─".repeat(width)
        } else {
            let label = format!("─ {} ─", label_parts.join(" • "));
            let padding = width.saturating_sub(display_width(&label));
            format!("{label}{}", "─".repeat(padding))
        };

        vec![Line::from(Span::styled(
            line,
            Style::default().add_modifier(Modifier::DIM),
        ))]
    }
}

#[derive(Clone, Debug)]
pub(super) struct AssistantHistoryCell {
    lines: Vec<Line<'static>>,
    is_stream_continuation: bool,
}

impl AssistantHistoryCell {
    pub(super) fn new(lines: Vec<Line<'static>>, is_stream_continuation: bool) -> Self {
        Self {
            lines,
            is_stream_continuation,
        }
    }
}

impl HistoryCell for AssistantHistoryCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        self.lines.clone()
    }

    fn is_stream_continuation(&self) -> bool {
        self.is_stream_continuation
    }

    fn is_assistant_stream_cell(&self) -> bool {
        true
    }
}

#[derive(Clone, Debug)]
pub(super) struct AssistantMarkdownHistoryCell {
    source: String,
    cwd: PathBuf,
}

impl AssistantMarkdownHistoryCell {
    pub(super) fn new(source: impl Into<String>, cwd: &Path) -> Self {
        Self {
            source: source.into(),
            cwd: cwd.to_path_buf(),
        }
    }
}

impl HistoryCell for AssistantMarkdownHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        crate::markdown::append_markdown(
            &self.source,
            Some(usize::from(width.max(1))),
            Some(self.cwd.as_path()),
            &mut lines,
        );
        lines
    }
}

#[derive(Clone, Debug)]
pub(super) struct ToolCallHistoryCell {
    pub(super) verb: &'static str,
    pub(super) tool_name: String,
    pub(super) call_id: String,
    pub(super) detail: Option<String>,
    pub(super) result: Option<ToolResultEnvelope>,
}

impl ToolCallHistoryCell {
    pub(super) fn new(
        verb: &'static str,
        tool_name: String,
        call_id: String,
        detail: Option<String>,
        result: Option<ToolResultEnvelope>,
    ) -> Self {
        Self {
            verb,
            tool_name,
            call_id,
            detail,
            result,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct ToolExplorationEntry {
    tool_name: String,
    call_id: String,
    detail: Option<String>,
    result: Option<ToolResultEnvelope>,
}

#[derive(Clone, Debug)]
pub(super) struct ToolExplorationHistoryCell {
    entries: Vec<ToolExplorationEntry>,
}

pub(super) fn is_exploration_tool_name(tool_name: &str) -> bool {
    matches!(tool_name, "list" | "read" | "search")
}

impl ToolExplorationHistoryCell {
    pub(super) fn new(tool_name: String, call_id: String, detail: Option<String>) -> Self {
        Self {
            entries: vec![ToolExplorationEntry {
                tool_name,
                call_id,
                detail,
                result: None,
            }],
        }
    }

    pub(super) fn contains_call_id(&self, call_id: &str) -> bool {
        self.entries.iter().any(|entry| entry.call_id == call_id)
    }

    pub(super) fn call_ids(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|entry| entry.call_id.clone())
            .collect()
    }

    pub(super) fn append_or_update(
        &mut self,
        tool_name: String,
        call_id: String,
        detail: Option<String>,
    ) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.call_id == call_id)
        {
            if entry.detail.is_none() {
                entry.detail = detail;
            }
            return;
        }
        self.entries.push(ToolExplorationEntry {
            tool_name,
            call_id,
            detail,
            result: None,
        });
    }

    pub(super) fn update_detail(&mut self, call_id: &str, detail: Option<String>) -> bool {
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.call_id == call_id)
        else {
            return false;
        };
        entry.detail = detail;
        true
    }

    pub(super) fn complete(&mut self, result: ToolResultEnvelope) -> bool {
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.call_id == result.call_id.to_string())
        else {
            return false;
        };
        entry.result = Some(result);
        true
    }

    pub(super) fn is_complete(&self) -> bool {
        self.entries.iter().all(|entry| entry.result.is_some())
    }
}

fn exploration_entry_title(tool_name: &str) -> &'static str {
    match tool_name {
        "list" => "List",
        "read" => "Read",
        "search" => "Search",
        _ => "Tool",
    }
}

fn exploration_entry_body(entry: &ToolExplorationEntry) -> String {
    entry
        .detail
        .as_deref()
        .filter(|detail| !detail.trim().is_empty())
        .unwrap_or(entry.tool_name.as_str())
        .to_owned()
}

fn render_exploration_detail_lines(
    entries: &[ToolExplorationEntry],
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut index = 0usize;
    while index < entries.len() {
        let entry = &entries[index];
        if entry.tool_name == "read" {
            let mut names = Vec::new();
            while index < entries.len() && entries[index].tool_name == "read" {
                let name = exploration_entry_body(&entries[index]);
                if !names.iter().any(|existing| existing == &name) {
                    names.push(name);
                }
                index += 1;
            }
            let body = names.join(", ");
            let rendered = render_wrapped_prefixed_entry("Read ", "     ", &body, width);
            lines.extend(
                rendered
                    .lines()
                    .map(|line| styled_exploration_line(line, "Read")),
            );
            continue;
        }

        let title = exploration_entry_title(&entry.tool_name);
        let body = exploration_entry_body(entry);
        let continuation = " ".repeat(display_width(title) + 1);
        let rendered =
            render_wrapped_prefixed_entry(&format!("{title} "), &continuation, &body, width);
        lines.extend(
            rendered
                .lines()
                .map(|line| styled_exploration_line(line, title)),
        );
        index += 1;
    }
    lines
}

fn styled_exploration_line(line: &str, title: &'static str) -> Line<'static> {
    let Some(rest) = line.strip_prefix(&format!("{title} ")) else {
        return Line::from(line.to_owned());
    };
    Line::from(vec![
        Span::styled(title, Style::default().fg(Color::Cyan)),
        Span::raw(" "),
        Span::raw(rest.to_owned()),
    ])
}

impl HistoryCell for ToolExplorationHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let status = if self.is_complete() {
            "Explored"
        } else {
            "Exploring"
        };
        let mut lines = vec![Line::from(vec![
            Span::styled("•", Style::default().add_modifier(Modifier::DIM)),
            Span::raw(" "),
            Span::styled(status, Style::default().add_modifier(Modifier::BOLD)),
        ])];
        let detail_width = width.saturating_sub(4).max(1);
        let details = render_exploration_detail_lines(&self.entries, detail_width);
        for (index, line) in details.into_iter().enumerate() {
            let prefix = if index == 0 { "  └ " } else { "    " };
            let mut spans = vec![Span::raw(prefix)];
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        }
        lines
    }
}

impl HistoryCell for ToolCallHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        render_tool_block_entry(
            self.verb,
            &self.tool_name,
            &self.call_id,
            self.detail.as_deref(),
            self.result.as_ref(),
            width,
        )
        .lines()
        .map(|line| Line::from(line.to_owned()))
        .collect()
    }
}
