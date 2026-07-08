use std::collections::VecDeque;
use std::path::Path;
use std::time::{Duration, Instant};

use ratatui::text::Line;

use crate::markdown_stream::MarkdownStreamCollector;

pub(super) mod chunking;
pub(super) mod commit_tick;
pub(super) mod controller;

struct QueuedLine {
    line: Line<'static>,
    enqueued_at: Instant,
}

pub(super) struct StreamState {
    pub(super) collector: MarkdownStreamCollector,
    queued_lines: VecDeque<QueuedLine>,
    pub(super) has_seen_delta: bool,
}

impl StreamState {
    pub(super) fn new(width: u16, cwd: &Path) -> Self {
        Self {
            collector: MarkdownStreamCollector::new(Some(usize::from(width.max(1))), cwd),
            queued_lines: VecDeque::new(),
            has_seen_delta: false,
        }
    }

    pub(super) fn clear(&mut self) {
        self.collector.clear();
        self.queued_lines.clear();
        self.has_seen_delta = false;
    }

    pub(super) fn step(&mut self) -> Vec<Line<'static>> {
        self.queued_lines
            .pop_front()
            .map(|queued| queued.line)
            .into_iter()
            .collect()
    }

    pub(super) fn drain_n(&mut self, max_lines: usize) -> Vec<Line<'static>> {
        let end = max_lines.min(self.queued_lines.len());
        self.queued_lines
            .drain(..end)
            .map(|queued| queued.line)
            .collect()
    }

    pub(super) fn drain_all(&mut self) -> Vec<Line<'static>> {
        self.queued_lines
            .drain(..)
            .map(|queued| queued.line)
            .collect()
    }

    pub(super) fn is_idle(&self) -> bool {
        self.queued_lines.is_empty()
    }

    pub(super) fn queued_len(&self) -> usize {
        self.queued_lines.len()
    }

    pub(super) fn queued_line_clones(&self) -> Vec<Line<'static>> {
        self.queued_lines
            .iter()
            .map(|queued| queued.line.clone())
            .collect()
    }

    pub(super) fn oldest_queued_age(&self, now: Instant) -> Option<Duration> {
        self.queued_lines
            .front()
            .map(|queued| now.saturating_duration_since(queued.enqueued_at))
    }

    pub(super) fn enqueue(&mut self, lines: Vec<Line<'static>>) {
        let now = Instant::now();
        self.queued_lines
            .extend(lines.into_iter().map(|line| QueuedLine {
                line,
                enqueued_at: now,
            }));
    }
}
