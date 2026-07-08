# Slash Command Audit — 2026-04-05

This note records the current `bt-tui` slash-command surface and the review frame we should use when evaluating it command by command.

It is intentionally a product and subsystem note, not a redesign spec.
The goal is to make the command surface easier to reason about before changing any behavior.

## Scope

This note is about the operator-facing slash commands in `bt-tui`.

Primary implementation references:

- [`crates/bt-tui/src/commands.rs`](../../crates/bt-tui/src/commands.rs)
- [`crates/bt-tui/src/command_actions.rs`](../../crates/bt-tui/src/command_actions.rs)
- [`crates/bt-tui/src/command_render.rs`](../../crates/bt-tui/src/command_render.rs)
- [`crates/bt-client/src/lib.rs`](../../crates/bt-client/src/lib.rs)

Relevant product and subsystem references:

- [`../subsystems/launcher-and-tui.md`](../subsystems/launcher-and-tui.md)
- [`../subsystems/tui-architecture.md`](../subsystems/tui-architecture.md)
- [`../subsystems/providers-auth-and-models.md`](../subsystems/providers-auth-and-models.md)
- [`tui-reference-review-20260402.md`](../development/tui-reference-review-20260402.md)

## Current Command Inventory

Canonical commands:

- `/help`
- `/status`
- `/models`
- `/doctor`
- `/mcp [reload]`
- `/session`
- `/inspect [session|tool <call-id>]`
- `/history [limit]`
- `/execution [limit]`
- `/raw [diff] [turn-id] [call <n>] [limit]`
- `/usage [limit]`
- `/queue [clear]`
- `/branches`
- `/tree`
- `/lineage`
- `/workflow`
- `/export [jsonl|html|sharegpt|otlp]`
- `/refresh`
- `/new`
- `/compact`
- `/resume [session-id]`
- `/spawn <objective>`
- `/branch`
- `/cancel`
- `/detach`
- `/steer <message>`
- `/approve [once|session|always]`
- `/deny [once|session|always]`
- `/connection [id]`
- `/model [model-id]`
- `/mode [standard|extended]`
- `/use <connection> [model]`

Aliases:

- `/r` -> `/refresh`
- `/delegate` -> `/spawn`
- `/fork` -> `/branch`
- `/stop` -> `/cancel`
- `/redirect` -> `/steer`
- `/yes` -> `/approve`
- `/no` -> `/deny`
- `/conn` -> `/connection`
- `/mod` -> `/model`

## Command Taxonomy

The current surface falls into a few clear product categories.

### 1. Readiness and configuration

These commands answer "can this harness run correctly, and with what model/backend state?"

- `/help`
- `/status`
- `/models`
- `/doctor`
- `/mcp`
- `/connection`
- `/model`
- `/mode`
- `/use`

This is the Hermes-style readiness story and should remain coherent as one family.

### 2. Session inspection and observability

These commands answer "what happened in this session, and how do I inspect it without mutating transcript history?"

- `/session`
- `/inspect`
- `/history`
- `/execution`
- `/raw`
- `/usage`
- `/queue`
- `/export`

This family is where Belltower's protocol honesty matters most.
These commands should expose canonical state, not local TUI inventions.

### 3. Session graph and branch navigation

These commands answer "where am I in the session/branch graph?"

- `/branches`
- `/tree`
- `/lineage`
- `/workflow`

These are graph views, not just alternate renderings of the same list.
They need crisp differentiation.

### 4. Session lifecycle and branching

These commands answer "what session or branch am I working in, and how do I move or create one?"

- `/refresh`
- `/new`
- `/resume`
- `/compact`
- `/spawn`
- `/branch`

This family should preserve explicit lifecycle boundaries.
It should never feel like hidden local TUI state mutation.

### 5. In-flight turn control

These commands answer "what can I do while a turn is active or paused?"

- `/cancel`
- `/detach`
- `/steer`
- `/approve`
- `/deny`

This family is behaviorally sensitive and should stay tightly aligned with runtime semantics.

## Structural Strengths

The current implementation has the right high-level shape.

- The command vocabulary lives in a central registry in `commands.rs`.
- Parsing and dispatch live in `command_actions.rs`.
- Rendering lives in `command_render.rs`.
- Most commands call a real server route through `bt-client`.
- Operator command results are persisted through `record_operator_command`.
- Completion and slash-menu metadata are centralized instead of being spread across input code.

That is a good foundation for modularity, maintainability, and scalability.

## Important Overlaps To Review Carefully

These are the places where the surface is coherent enough to be useful, but potentially redundant or confusing enough to justify a command-by-command review.

### `/session` vs `/inspect session`

Today these are the same handler.

That may be acceptable, but it is worth deciding explicitly whether:

- `/inspect` is the universal drill-down namespace and `/session` is convenience sugar
- or `/session` should own the high-level summary while `/inspect session` should become deeper

### `/connection`, `/model`, and `/use`

This overlap is intentional, but it needs a crisp operator story.

Likely intended roles:

- `/connection` is a direct connection switch or current-value check
- `/model` is a direct model switch or current-value check
- `/use` is the guided combined flow

If that is the real design, docs and help should reinforce it.

### `/history`, `/execution`, `/raw`, and `/usage`

These are all observability commands, but they sit at different depths:

- `/history` is turn-level recency
- `/execution` is structured execution metadata
- `/raw` is low-level provider/raw-chunk inspection
- `/usage` is token/cost/latency accounting

The command set is fine if those boundaries stay sharp.

### `/branches`, `/tree`, `/lineage`, and `/workflow`

These all operate on graph context, but they are not interchangeable.

Current likely roles:

- `/branches` is branch inventory inside one session
- `/tree` is branch tree plus related sessions
- `/lineage` is cross-session ancestry
- `/workflow` is a broader multi-session runtime/status view

That distinction should stay explicit.

### `/mcp`, `/queue`, and `/export`

These are mixed-surface commands because they include both inspection and mutation:

- `/mcp` and `/mcp reload`
- `/queue` and `/queue clear`
- `/export otlp` and `/export otlp push`

This is acceptable, but these sub-actions should feel intentional rather than bolted on.

## In-Flight Policy Boundary

One of the stronger current design choices is that only a limited subset of commands is allowed during an active request.

Allowed during request today:

- `/status`
- `/models`
- `/help`
- `/session`
- `/inspect`
- `/history`
- `/execution`
- `/raw`
- `/usage`
- `/queue`
- `/branches`
- `/tree`
- `/lineage`
- `/workflow`
- `/export`
- `/detach`
- `/cancel`
- `/steer`

That policy makes sense.
It preserves a clean boundary between sideband inspection/control and commands that would mutate session identity or configuration mid-turn.

## Review Criteria

For each command, the right questions are:

1. Is the command name obvious and discoverable?
2. Is its purpose distinct from nearby commands?
3. Does it reflect canonical server/runtime state rather than local TUI guesswork?
4. Does its output match the product model in `launcher-and-tui.md` and `tui-architecture.md`?
5. Is it safe and sensible during an active request?
6. Does it belong as a top-level slash command, or should it be folded into another command family?
7. Does it reinforce a coherent operator mental model?

## Recommended Review Order

The best review order is:

1. `/help`
2. `/status`, `/models`, `/doctor`, `/mcp`
3. `/connection`, `/model`, `/mode`, `/use`
4. `/session`, `/inspect`
5. `/history`, `/execution`, `/raw`, `/usage`, `/queue`, `/export`
6. `/branches`, `/tree`, `/lineage`, `/workflow`
7. `/refresh`, `/new`, `/resume`, `/compact`, `/spawn`, `/branch`
8. `/cancel`, `/detach`, `/steer`, `/approve`, `/deny`

This order is intentional.

- `/help` defines discoverability.
- readiness and model-selection commands define the operator's first mental model.
- inspection commands define how Belltower explains itself.
- lifecycle and in-flight controls should be reviewed only after the read paths are clear.

## Working Conclusion

The slash-command subsystem is in decent shape structurally.
The main remaining question is not "is there a command system?" but "is the command taxonomy crisp enough that each command obviously earns its place?"

That is what the next review pass should decide.
