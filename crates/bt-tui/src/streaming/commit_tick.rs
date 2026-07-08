use super::chunking::{AdaptiveChunkingPolicy, ChunkingMode, DrainPlan, QueueSnapshot};
use super::controller::StreamController;
use crate::history_cell::SharedHistoryCell;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommitTickScope {
    AnyMode,
    CatchUpOnly,
}

#[derive(Default)]
pub(crate) struct CommitTickOutput {
    pub(crate) cells: Vec<SharedHistoryCell>,
    pub(crate) has_controller: bool,
    pub(crate) all_idle: bool,
}

pub(crate) fn run_commit_tick(
    policy: &mut AdaptiveChunkingPolicy,
    stream_controller: Option<&mut StreamController>,
    scope: CommitTickScope,
    now: Instant,
) -> CommitTickOutput {
    let snapshot = stream_queue_snapshot(stream_controller.as_deref(), now);
    let decision = policy.decide(snapshot, now);
    if scope == CommitTickScope::CatchUpOnly && decision.mode != ChunkingMode::CatchUp {
        return CommitTickOutput::default();
    }
    apply_commit_tick_plan(decision.drain_plan, stream_controller)
}

fn stream_queue_snapshot(
    stream_controller: Option<&StreamController>,
    now: Instant,
) -> QueueSnapshot {
    let mut queued_lines = 0usize;
    let mut oldest_age: Option<Duration> = None;
    if let Some(controller) = stream_controller {
        queued_lines += controller.queued_lines();
        oldest_age = max_duration(oldest_age, controller.oldest_queued_age(now));
    }
    QueueSnapshot {
        queued_lines,
        oldest_age,
    }
}

fn apply_commit_tick_plan(
    drain_plan: DrainPlan,
    stream_controller: Option<&mut StreamController>,
) -> CommitTickOutput {
    let mut output = CommitTickOutput {
        cells: Vec::new(),
        has_controller: false,
        all_idle: true,
    };
    if let Some(controller) = stream_controller {
        output.has_controller = true;
        let (cell, idle) = match drain_plan {
            DrainPlan::Single => controller.on_commit_tick(),
            DrainPlan::Batch(max_lines) => controller.on_commit_tick_batch(max_lines),
        };
        if let Some(cell) = cell {
            output.cells.push(cell);
        }
        output.all_idle &= idle;
    }
    output
}

fn max_duration(lhs: Option<Duration>, rhs: Option<Duration>) -> Option<Duration> {
    match (lhs, rhs) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}
