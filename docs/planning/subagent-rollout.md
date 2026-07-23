# Subagent Rollout Plan

This document turns the subagent/session-graph design into a practical implementation decision aid.

It should be read after:

- [`../architecture/subagents-and-session-graphs.md`](../architecture/subagents-and-session-graphs.md)
- [`./current-sprint.md`](./current-sprint.md)
- [`./target-implementation-goals.md`](./target-implementation-goals.md)

## Purpose

The point of this document is to record the safe rollout boundary for subagent work.

It is to answer:

- what must be true before Belltower spawns child sessions
- what the smallest safe implementation slice is
- when it is better to keep building inspection and reproducibility first

## Current State

Belltower already has some of the necessary substrate:

- explicit session records with parent-session lineage fields
- explicit branch records and branch trees
- canonical turn lifecycle events
- canonical event-store-backed session, history, trace, branch, and tree inspection
- post-commit tracing mirror
- structured approval and queue state
- canonical delegation events for spawn, handoff, and result-import intent
- a manual child-session spawn path via runtime, protocol, server, and TUI (`/spawn`)
- typed durable parent-child mailbox events and projections
- model-facing `spawn_agent`, `send_agent_message`, `list_agents`, and
  `wait_agent` tools for extended-tool sessions
- per-child configured connection and model selection
- bounded depth and lineage size, explicit shared-workspace consent, and
  approval-gated autonomous spawn

What it does not yet have is a complete workflow framework.

Missing pieces include:

- a workflow-level inspection surface beyond single-session related-session summaries
- worktree policy and lifecycle
- isolated-worktree ownership and lifecycle
- explicit inherited tool/budget policy beyond ordinary session-local state
- a richer artifact-first return contract beyond typed messages with artifact refs
- higher-order scheduling, role, or recursive delegation policy

## Recommendation

Keep autonomous execution bounded to the implemented child-session and mailbox
contract. Do not broaden it into a general scheduler until dogfooding shows a
measured need.

The next safe choices are:

1. dogfood mixed-model parent-child coordination on real tasks
2. evaluate isolated-worktree lifecycle before concurrent coding children
3. add richer return artifacts only when typed result/error messages and
   existing artifact refs are insufficient
4. introduce higher-order workflow policy in a separate composition layer, not
   by expanding per-session runtime ownership

This preserves Belltower's core differentiator:

- autonomous work remains understandable because the telemetry model lands before the autonomy layer

## Required Prerequisites

Before Belltower can safely spawn child sessions, the following should exist.

### RP-1. Delegation Event Taxonomy

Canonical events for:

- spawn request
- child session created
- handoff recorded
- result imported
- result rejected or abandoned

### RP-2. Workflow Inspection Surface

The operator should be able to inspect a workflow spanning multiple sessions.

Likely surface:

- `/workflow`

At minimum it should answer:

- which sessions belong to the same higher-level workflow
- which session spawned which child session
- where the child originated from in parent branch/turn space
- whether the child completed, failed, or was abandoned

### RP-3. Worktree Policy

Before delegated coding work:

- decide default worktree isolation behavior
- define worktree lifecycle events and cleanup policy
- define how worktree identity is stored and inspected

### RP-4. Structured Return Artifact

Child sessions should return a structured artifact, not just transcript text.

Minimum return shape:

- status
- summary
- changed files or artifact references
- child session ID
- branch/worktree handle
- optional export or trace handle

### RP-5. Policy and Budget Contract

Delegated sessions need explicit limits.

At minimum:

- tool scope
- approval policy
- model/provider selection
- token/time budget
- filesystem or worktree scope

## Proposed Implementation Order

### Slice A. Delegation-Ready Inspection

This is the smallest next slice if we want to prepare for subagents without committing to them.

Implement:

- `/workflow`
- workflow DTOs over server/client/runtime
- stronger related-session inspection
- delegation-oriented event names reserved in the taxonomy

Status: complete. `/workflow` is now the workflow-oriented inspection surface on top of canonical lineage and session metrics.

### Slice B. Delegation Events Without Autonomous Spawn

Implement canonical events and storage/query support for:

- spawn requested
- child session created
- handoff summary recorded
- result imported

Status: complete for spawn and handoff, partial for result import.

This validates the telemetry model before more autonomous behavior lands.

### Slice C. Manual Child-Session Spawn

Add a product surface for deliberate child-session creation:

- operator-triggered
- explicit objective
- explicit parent branch/turn linkage
- explicit model/worktree policy

Status: complete for manual operator spawn.

The current product surface is `/spawn <objective>`, which:

- creates a child session
- links it to the parent session, branch, and latest branch turn by default
- records `session.spawn.requested`, `session.spawned`, and `session.handoff.recorded`
- keeps the operator in the parent session and makes the child resumable

What is still missing from this slice is explicit worktree policy and richer model/tool budget controls.

### Slice D. Bounded Autonomous Spawn

Status: complete.

The implemented slice:

- exposes model-facing spawn only in extended tool mode
- always requires human approval and `allow_shared_workspace: true`
- allows an explicit configured connection and model per child
- limits messages to direct parent-child edges
- limits depth to four and descendants to eight per lineage
- gives every child independent queue, approval, cancel, steer, budget, and
  settings state
- persists paired source/destination mailbox evidence atomically
- distinguishes `notify` from FIFO `wake` delivery
- returns typed progress, result, or error messages and reports interrupted
  claimed turns without retrying side effects after restart

## Decision Guidance

Use this rule when choosing what to build next.

### Build Subagent Foundations Next If

- the product goal is to start exercising multi-session workflows soon
- session lineage inspection is already the bottleneck
- the team wants to validate worktree and delegation semantics early

### Defer Subagent Foundations Briefly If

- export and usage inspection are still missing
- the operator cannot yet inspect the same work consistently across transcript, trace, and export views
- current queue/approval surfaces still need polish for day-to-day use

Given the current repo state, the best immediate next work is now:

1. dogfood the bounded contract with heterogeneous configured models
2. design worktree policy and lifecycle before concurrent coding delegation
3. evaluate return-artifact and higher-order workflow needs from observed traces

That order keeps the observability story ahead of the autonomy story.

## Human Checkpoints For The Future Subagent Slice

The manual checkpoints for this slice are:

1. create a parent session
2. create a child session from a specific parent turn
3. inspect the parent with `/session`, `/lineage`, and `/workflow`
4. inspect the child with `/session`, `/history`, and `/execution`
5. confirm the parent-child relationship is visible from both sides
6. confirm approvals, queue state, and cancellations remain isolated
7. confirm exported traces and canonical events agree on lineage and outcome
8. spawn children on different configured providers/models and confirm each
   turn captures the child's own immutable settings revision
9. exchange messages in both directions, restart between delivery and claim,
   and confirm no message is duplicated or silently lost

## Practical Conclusion

The design is now clear enough that we do not need to guess later:

- subagents should be child sessions
- child sessions should usually use isolated worktrees
- lineage and delegation must be explicit in canonical events and inspection

The remaining decisions concern isolated execution and higher-order workflow
policy, not the canonical child-session or communication model.
