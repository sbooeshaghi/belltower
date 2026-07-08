use super::StreamState;
use crate::TranscriptDensity;
use crate::history_cell::{AssistantHistoryCell, SharedHistoryCell};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) struct StreamController {
    state: StreamState,
    header_emitted: bool,
}

impl StreamController {
    pub(crate) fn new(width: u16, cwd: &Path) -> Self {
        Self {
            state: StreamState::new(width, cwd),
            header_emitted: false,
        }
    }

    pub(crate) fn push(
        &mut self,
        delta: &str,
        _show_reasoning: bool,
        _transcript_density: TranscriptDensity,
    ) -> bool {
        if !delta.is_empty() {
            self.state.has_seen_delta = true;
        }
        self.state.collector.push_delta(delta);
        if delta.contains('\n') {
            let committed = self.state.collector.commit_complete_lines();
            if !committed.is_empty() {
                self.state.enqueue(committed);
                return true;
            }
        }
        false
    }

    pub(crate) fn finalize(
        &mut self,
        _show_reasoning: bool,
        _transcript_density: TranscriptDensity,
    ) -> Option<SharedHistoryCell> {
        let remaining = self.state.collector.finalize_and_drain();
        if !remaining.is_empty() {
            self.state.enqueue(remaining);
        }
        let lines = self.state.drain_all();
        self.state.clear();
        self.emit(lines)
    }

    pub(crate) fn on_commit_tick(&mut self) -> (Option<SharedHistoryCell>, bool) {
        let step = self.state.step();
        (self.emit(step), self.state.is_idle())
    }

    pub(crate) fn on_commit_tick_batch(
        &mut self,
        max_lines: usize,
    ) -> (Option<SharedHistoryCell>, bool) {
        let step = self.state.drain_n(max_lines.max(1));
        (self.emit(step), self.state.is_idle())
    }

    pub(crate) fn queued_lines(&self) -> usize {
        self.state.queued_len()
    }

    pub(crate) fn oldest_queued_age(&self, now: Instant) -> Option<Duration> {
        self.state.oldest_queued_age(now)
    }

    pub(crate) fn live_tail_lines(
        &self,
        _show_reasoning: bool,
        _transcript_density: TranscriptDensity,
    ) -> Vec<ratatui::text::Line<'static>> {
        let mut lines = self.state.queued_line_clones();
        lines.extend(self.state.collector.live_tail_lines());
        lines
    }
    pub(crate) fn raw_text(&self) -> &str {
        self.state.collector.raw_text()
    }

    fn emit(&mut self, lines: Vec<ratatui::text::Line<'static>>) -> Option<SharedHistoryCell> {
        if lines.is_empty() {
            return None;
        }
        let is_stream_continuation = self.header_emitted;
        self.header_emitted = true;
        Some(Arc::new(AssistantHistoryCell::new(
            lines,
            is_stream_continuation,
        )))
    }
}
