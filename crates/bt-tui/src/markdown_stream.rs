use std::path::{Path, PathBuf};

use ratatui::text::Line;

use crate::markdown;

#[derive(Clone, Debug, Default)]
pub(super) struct MarkdownStreamCollector {
    buffer: String,
    committed_line_count: usize,
    width: Option<usize>,
    cwd: PathBuf,
}

impl MarkdownStreamCollector {
    pub(super) fn new(width: Option<usize>, cwd: &Path) -> Self {
        Self {
            buffer: String::new(),
            committed_line_count: 0,
            width,
            cwd: cwd.to_path_buf(),
        }
    }

    pub(super) fn clear(&mut self) {
        self.buffer.clear();
        self.committed_line_count = 0;
    }

    pub(super) fn push_delta(&mut self, delta: &str) {
        self.buffer.push_str(delta);
    }

    pub(super) fn raw_text(&self) -> &str {
        &self.buffer
    }

    pub(super) fn commit_complete_lines(&mut self) -> Vec<Line<'static>> {
        let source = self.buffer.clone();
        let Some(last_newline_idx) = source.rfind('\n') else {
            return Vec::new();
        };
        let source = source[..=last_newline_idx].to_string();
        let mut rendered = Vec::new();
        markdown::append_markdown(&source, self.width, Some(self.cwd.as_path()), &mut rendered);
        let mut complete_line_count = rendered.len();
        if complete_line_count > 0 && is_blank_line_spaces_only(&rendered[complete_line_count - 1])
        {
            complete_line_count -= 1;
        }

        if self.committed_line_count >= complete_line_count {
            return Vec::new();
        }

        let out = rendered[self.committed_line_count..complete_line_count].to_vec();
        self.committed_line_count = complete_line_count;
        out
    }

    pub(super) fn finalize_and_drain(&mut self) -> Vec<Line<'static>> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let mut source = self.buffer.clone();
        if !source.ends_with('\n') {
            source.push('\n');
        }
        let mut rendered = Vec::new();
        markdown::append_markdown(&source, self.width, Some(self.cwd.as_path()), &mut rendered);
        let out = if self.committed_line_count >= rendered.len() {
            Vec::new()
        } else {
            rendered[self.committed_line_count..].to_vec()
        };
        self.clear();
        out
    }

    pub(super) fn live_tail_lines(&self) -> Vec<Line<'static>> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let mut rendered = Vec::new();
        markdown::append_markdown(
            &self.buffer,
            self.width,
            Some(self.cwd.as_path()),
            &mut rendered,
        );
        if self.committed_line_count >= rendered.len() {
            Vec::new()
        } else {
            rendered[self.committed_line_count..].to_vec()
        }
    }
}

fn is_blank_line_spaces_only(line: &Line<'_>) -> bool {
    if line.spans.is_empty() {
        return true;
    }
    line.spans
        .iter()
        .all(|span| span.content.is_empty() || span.content.chars().all(|ch| ch == ' '))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_complete_lines_collapses_repeated_blank_lines() {
        let mut collector = MarkdownStreamCollector::new(Some(80), Path::new("/tmp"));
        collector.push_delta("first\n\n\n\nsecond\n");
        let committed = collector.commit_complete_lines();
        let rendered = committed
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(rendered, "first\n\nsecond");
    }

    #[test]
    fn live_tail_preserves_soft_breaks_without_duplication() {
        let mut collector = MarkdownStreamCollector::new(Some(80), Path::new("/tmp"));
        collector.push_delta("first line\n");
        let committed = collector.commit_complete_lines();
        assert_eq!(
            committed.iter().map(Line::to_string).collect::<Vec<_>>(),
            vec!["first line"]
        );

        collector.push_delta("second line");
        let live = collector.live_tail_lines();
        assert_eq!(
            live.iter().map(Line::to_string).collect::<Vec<_>>(),
            vec!["second line"]
        );

        let final_lines = collector.finalize_and_drain();
        assert_eq!(
            final_lines.iter().map(Line::to_string).collect::<Vec<_>>(),
            vec!["second line"]
        );
    }
}
