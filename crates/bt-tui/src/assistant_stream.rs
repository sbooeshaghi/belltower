use super::*;

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct AssistantStreamCollector {
    raw_buffer: String,
    committed_raw_len: usize,
    committed_line_count: usize,
}

impl AssistantStreamCollector {
    pub(super) fn has_committed_output(&self) -> bool {
        self.committed_raw_len > 0 || self.committed_line_count > 0
    }

    pub(super) fn is_empty(&self) -> bool {
        self.raw_buffer.is_empty()
    }

    pub(super) fn push_delta(&mut self, delta: &str) {
        self.raw_buffer.push_str(delta);
    }

    pub(super) fn append_canonical_text(&mut self, text: &str) {
        self.raw_buffer.push_str(text);
    }

    pub(super) fn raw_text(&self) -> &str {
        &self.raw_buffer
    }

    pub(super) fn raw_live_text(&self) -> Option<&str> {
        (self.committed_raw_len < self.raw_buffer.len())
            .then_some(&self.raw_buffer[self.committed_raw_len..])
    }

    pub(super) fn committed_raw_len(&self) -> usize {
        self.committed_raw_len
    }

    pub(super) fn has_uncommitted_content(&self) -> bool {
        self.raw_live_text().is_some()
    }

    pub(super) fn commit_complete_lines(
        &mut self,
        show_reasoning: bool,
        transcript_density: TranscriptDensity,
        view_width: u16,
    ) -> Vec<String> {
        let Some(last_newline_idx) = self.raw_buffer.rfind('\n') else {
            return Vec::new();
        };
        let complete_raw_end = last_newline_idx + 1;
        if complete_raw_end <= self.committed_raw_len {
            return Vec::new();
        }

        let mut rendered_lines = self.render_lines_for(
            &self.raw_buffer[..complete_raw_end],
            show_reasoning,
            transcript_density,
            view_width,
        );
        let mut complete_line_count = rendered_lines.len();
        if complete_line_count > 0 && rendered_lines[complete_line_count - 1].trim().is_empty() {
            complete_line_count -= 1;
        }

        if self.committed_line_count >= complete_line_count {
            self.committed_raw_len = complete_raw_end;
            return Vec::new();
        }

        let out = rendered_lines
            .drain(self.committed_line_count..complete_line_count)
            .collect::<Vec<_>>();
        self.committed_raw_len = complete_raw_end;
        self.committed_line_count = complete_line_count;
        out
    }

    pub(super) fn finalize_and_drain(
        &mut self,
        show_reasoning: bool,
        transcript_density: TranscriptDensity,
        view_width: u16,
    ) -> Vec<String> {
        if self.raw_buffer.is_empty() {
            return Vec::new();
        }

        let mut source = self.raw_buffer.clone();
        if !source.ends_with('\n') {
            source.push('\n');
        }
        let rendered_lines =
            self.render_lines_for(&source, show_reasoning, transcript_density, view_width);
        let out = if self.committed_line_count >= rendered_lines.len() {
            Vec::new()
        } else {
            rendered_lines[self.committed_line_count..].to_vec()
        };
        self.committed_raw_len = self.raw_buffer.len();
        self.committed_line_count = rendered_lines.len();
        out
    }

    pub(super) fn live_rendered_text(
        &self,
        show_reasoning: bool,
        transcript_density: TranscriptDensity,
        view_width: u16,
    ) -> Option<String> {
        if self.raw_buffer.is_empty() {
            return None;
        }

        let rendered_lines = self.render_lines_for(
            &self.raw_buffer,
            show_reasoning,
            transcript_density,
            view_width,
        );
        if self.committed_line_count >= rendered_lines.len() {
            return None;
        }
        let text = rendered_lines[self.committed_line_count..].join("\n");
        (!text.is_empty()).then_some(text)
    }

    fn render_lines_for(
        &self,
        text: &str,
        show_reasoning: bool,
        transcript_density: TranscriptDensity,
        view_width: u16,
    ) -> Vec<String> {
        render_message_with_options(
            &Message::text(Role::Assistant, text.to_owned()),
            show_reasoning,
            transcript_density,
            view_width,
        )
        .lines()
        .map(|line| line.to_owned())
        .collect()
    }
}
