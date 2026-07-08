use std::time::{Duration, Instant};

const ENTER_QUEUE_DEPTH_LINES: usize = 8;
const ENTER_OLDEST_AGE: Duration = Duration::from_millis(120);
const EXIT_QUEUE_DEPTH_LINES: usize = 2;
const EXIT_OLDEST_AGE: Duration = Duration::from_millis(40);
const EXIT_HOLD: Duration = Duration::from_millis(250);
const REENTER_CATCH_UP_HOLD: Duration = Duration::from_millis(250);
const SEVERE_QUEUE_DEPTH_LINES: usize = 64;
const SEVERE_OLDEST_AGE: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ChunkingMode {
    #[default]
    Smooth,
    CatchUp,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct QueueSnapshot {
    pub(super) queued_lines: usize,
    pub(super) oldest_age: Option<Duration>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DrainPlan {
    Single,
    Batch(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChunkingDecision {
    pub(super) mode: ChunkingMode,
    pub(super) entered_catch_up: bool,
    pub(super) drain_plan: DrainPlan,
}

#[derive(Debug, Default)]
pub(crate) struct AdaptiveChunkingPolicy {
    mode: ChunkingMode,
    below_exit_threshold_since: Option<Instant>,
    last_catch_up_exit_at: Option<Instant>,
}

impl AdaptiveChunkingPolicy {
    pub(crate) fn reset(&mut self) {
        self.mode = ChunkingMode::Smooth;
        self.below_exit_threshold_since = None;
        self.last_catch_up_exit_at = None;
    }

    pub(crate) fn decide(&mut self, snapshot: QueueSnapshot, now: Instant) -> ChunkingDecision {
        if snapshot.queued_lines == 0 {
            self.note_catch_up_exit(now);
            self.mode = ChunkingMode::Smooth;
            self.below_exit_threshold_since = None;
            return ChunkingDecision {
                mode: self.mode,
                entered_catch_up: false,
                drain_plan: DrainPlan::Single,
            };
        }

        let entered_catch_up = match self.mode {
            ChunkingMode::Smooth => self.maybe_enter_catch_up(snapshot, now),
            ChunkingMode::CatchUp => {
                self.maybe_exit_catch_up(snapshot, now);
                false
            }
        };

        let drain_plan = match self.mode {
            ChunkingMode::Smooth => DrainPlan::Single,
            ChunkingMode::CatchUp => DrainPlan::Batch(snapshot.queued_lines.max(1)),
        };

        ChunkingDecision {
            mode: self.mode,
            entered_catch_up,
            drain_plan,
        }
    }

    fn maybe_enter_catch_up(&mut self, snapshot: QueueSnapshot, now: Instant) -> bool {
        if !should_enter_catch_up(snapshot) {
            return false;
        }
        if self.reentry_hold_active(now) && !is_severe_backlog(snapshot) {
            return false;
        }
        self.mode = ChunkingMode::CatchUp;
        self.below_exit_threshold_since = None;
        self.last_catch_up_exit_at = None;
        true
    }

    fn maybe_exit_catch_up(&mut self, snapshot: QueueSnapshot, now: Instant) {
        if !should_exit_catch_up(snapshot) {
            self.below_exit_threshold_since = None;
            return;
        }

        match self.below_exit_threshold_since {
            Some(since) if now.saturating_duration_since(since) >= EXIT_HOLD => {
                self.mode = ChunkingMode::Smooth;
                self.below_exit_threshold_since = None;
                self.last_catch_up_exit_at = Some(now);
            }
            Some(_) => {}
            None => {
                self.below_exit_threshold_since = Some(now);
            }
        }
    }

    fn note_catch_up_exit(&mut self, now: Instant) {
        if self.mode == ChunkingMode::CatchUp {
            self.last_catch_up_exit_at = Some(now);
        }
    }

    fn reentry_hold_active(&self, now: Instant) -> bool {
        self.last_catch_up_exit_at
            .is_some_and(|exit| now.saturating_duration_since(exit) < REENTER_CATCH_UP_HOLD)
    }
}

fn should_enter_catch_up(snapshot: QueueSnapshot) -> bool {
    snapshot.queued_lines >= ENTER_QUEUE_DEPTH_LINES
        || snapshot
            .oldest_age
            .is_some_and(|age| age >= ENTER_OLDEST_AGE)
}

fn should_exit_catch_up(snapshot: QueueSnapshot) -> bool {
    snapshot.queued_lines <= EXIT_QUEUE_DEPTH_LINES
        && snapshot.oldest_age.is_none_or(|age| age <= EXIT_OLDEST_AGE)
}

fn is_severe_backlog(snapshot: QueueSnapshot) -> bool {
    snapshot.queued_lines >= SEVERE_QUEUE_DEPTH_LINES
        || snapshot
            .oldest_age
            .is_some_and(|age| age >= SEVERE_OLDEST_AGE)
}
