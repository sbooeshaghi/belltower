# Subagents and Session Graphs

This document defines how Belltower should model subagents and related-session workflows.

It exists because Belltower's long-term value is not just "run one agent turn well."
It is "make autonomous and delegated work inspectable, reproducible, and auditable."

This document should be read with:

- [`./overview.md`](./overview.md)
- [`../subsystems/session-runtime-and-agent.md`](../subsystems/session-runtime-and-agent.md)
- [`../subsystems/telemetry-and-exports.md`](../subsystems/telemetry-and-exports.md)
- [`../planning/target-implementation-goals.md`](../planning/target-implementation-goals.md)

## Summary

The core rule is:

- a branch is a speculative continuation inside one session
- a subagent is a child session with its own runtime state and telemetry

Belltower may eventually "spawn itself," but that should mean:

- create a child session
- attach explicit lineage to the parent session, branch, and turn
- optionally create an isolated git worktree for that child session
- run the child session with its own queue, approvals, controls, and execution timeline
- return a structured result back to the parent session

This keeps branching, delegation, and telemetry understandable instead of collapsing them into one opaque transcript.

## Why Branches Are Not Enough

Branches and subagents solve different problems.

### Branches

Use a branch when:

- one operator thread wants to try another path
- work remains sequential
- approvals and queue state are still part of one session
- one transcript is still the right unit of work

### Child Sessions

Use a child session when:

- work should proceed independently
- a delegated worker needs its own tool loop
- approvals should be isolated from the parent
- queue, cancel, and steer must not interfere with the parent session
- a separate working directory or git worktree is needed
- the parent needs a summarized return artifact rather than "more transcript"

If Belltower treats true delegated work as "just another branch," the control model and telemetry model both become ambiguous.

## Canonical Model

Subagents should be represented as a session graph.

Each child session should carry:

- `parent_session_id`
- `parent_branch_id`
- `parent_turn_id`
- its own `session_id`

This is already aligned with the current session model, which can represent parent-child lineage.

The important semantic rule is:

- branch lineage answers "where in this session tree did work fork?"
- session lineage answers "which other sessions participated in the same higher-level workflow?"

These are related, but they are not the same thing.
That distinction should eventually be enforced at the type level, not only in prose:

- `BranchId` remains scoped to one `SessionId`
- child sessions get their own `SessionId` plus explicit parent foreign keys
- code should make it difficult to accidentally treat a child session as "just another branch"

## Control and Runtime Isolation

Each child session must own its own:

- message queue
- approval state
- cancel state
- steer queue
- turn lifecycle
- turn-scoped exported traces
- provider/tool/MCP execution state

The parent session may inspect or control the child session, but it should not share mutable runtime state with it.

The practical consequence is:

- parent and child sessions can be rendered in one workflow view
- but they must still remain independently replayable and independently cancellable

Workflow coordination should also remain out of `bt-runtime`.
When it lands, a dedicated composition layer such as `bt-workflow` is a better fit than letting subagent orchestration leak into the per-session runtime crate.

## Filesystem and Worktree Model

For coding agents, the safe default is:

- parent session may stay in the active project worktree
- child sessions should run in an isolated git worktree by default

This reduces several failure modes:

- file races between parent and child
- approval ambiguity around which session changed what
- impossible-to-reproduce mixed diffs
- accidental overwrite of operator changes

### Default Policy

- no shared worktree for autonomous child sessions by default
- shared worktree only as an explicit unsafe or advanced mode
- worktree identity should be part of session metadata and inspection output

This is especially important if Belltower later supports multiple concurrent delegated sessions.

## Telemetry Model

The canonical telemetry rule remains unchanged:

- the typed session event log is the source of truth
- `tracing` and OTEL are mirrors over committed events

For subagents, this means:

- each child session keeps its own canonical event log and its own turn-scoped exported traces
- parent-child relationships are represented in the canonical store
- spawn and return edges are explicit events, not inferred from transcript text

### Trace Shape

The preferred runtime shape is:

- parent session correlation container
- parent turn trace
- child session correlation container with its own turn traces

The relationship between them should be represented by lineage metadata and trace links, not by forcing every delegated session into one giant trace.

That keeps each session replayable and exportable on its own while still allowing workflow-level inspection later.

## Required Canonical Events

Belltower now has explicit delegation events in the canonical taxonomy:

- `session.spawn.requested`
- `session.spawned`
- `session.handoff.recorded`
- `session.result.imported`
- `session.result.rejected`

These should capture:

- parent session, branch, and turn identity
- child session identity
- objective or handoff summary
- requested budget and policy
- worktree identity if applicable
- final result summary and artifact references

The important rule is:

- session lineage must be represented as canonical events and canonical session metadata
- never as a hidden runtime side channel

Current manual spawn preserves that rule atomically. The child session row,
initial settings snapshot, root branch, parent `session.spawn.requested`, child
`session.started` and `session.handoff.recorded`, and parent `session.spawned`
either all commit or all roll back. The parent turn id is causal provenance and
does not grant permission to append new live evidence to a completed turn.

## Spawn Contract

Spawning a child session should require a structured contract, not just a free-form prompt.

At minimum that contract should include:

- objective
- parent handoff summary
- project root
- working directory or worktree
- connection and model
- instruction bundle
- tool allowlist or policy
- MCP scope
- budget limits
- expected return artifact

That return artifact should itself be structured.

At minimum it should support:

- summary
- outcome status
- relevant files or artifacts
- branch or worktree reference
- child session ID
- trace/session export handles if needed

## Approval Model

Approvals must not silently blur across parent and child sessions.

Rules:

- child sessions own their own pending approvals
- the parent may inspect them
- parent-level approval inheritance must be explicit and policy-based
- broad approvals should not automatically leak into delegated sessions by default

This matters because autonomous work should remain auditable.
If a child session used a dangerous tool, the operator must be able to answer which session approved it, under what scope, and why.

## UX Model

The future operator surface should distinguish:

- branch navigation
- related-session navigation
- delegated-work inspection

Likely commands or views:

- `/session`
- `/history`
- `/execution`
- `/branches`
- `/tree`
- `/workflow`
- future `/subagents`

The important UX rule is:

- a workflow view may unify parent and child sessions
- but each session must still be inspectable independently

## Recommended Rollout

Subagent work should land in phases.

### Phase 1. Lineage-Ready Inspection

This phase is mostly in place already.

- parent/child metadata exists in the session model
- inspection surfaces can show related sessions
- branch and tree inspection exist

### Phase 2. Explicit Delegation Events

Before autonomous spawning, add canonical events for:

- spawn request
- child session creation
- handoff summary
- result import

Status: mostly in place. Spawn and handoff events now exist in the canonical log; result import is reserved in the taxonomy but not yet a surfaced workflow feature.

### Phase 3. Manual Child-Session Spawn

Allow an operator or parent session to create a child session intentionally, without yet making autonomous spawning the default.

This is the safest first execution slice because:

- the telemetry model is exercised
- lineage and inspection surfaces can be validated
- worktree policy can be tested

Status: mostly in place.

The current manual operator surface is `/spawn [--connection <id>] [--model <model-id>] <objective>`. It creates a child session, links it to the parent session, branch, and latest branch turn by default, records canonical spawn and handoff events, and leaves the operator in the parent session so the child can be resumed explicitly.

If `--connection` or `--model` are omitted, runtime inheritance remains the source of truth: the child inherits the parent connection, and it inherits the parent model only when the connection is unchanged. This keeps model selection explicit when the operator intentionally assigns a child session to a different provider.

### Phase 4. Autonomous Child Sessions

Only after the spawn contract, lineage events, and inspection surfaces are solid should Belltower allow one session to autonomously spawn another.

The `/spawn` command is therefore not the same thing as an agent-facing `delegate` tool. `delegate` still needs execution policy, worktree isolation, approval inheritance rules, depth limits, budget behavior, and a typed result contract before it should become part of the model-facing toolkit.

## Design Rules

Every implementation should preserve these rules:

1. A branch is not a subagent.
2. A subagent is a child session, not a hidden nested transcript.
3. Parent and child sessions must be independently replayable.
4. Parent-child relationships must be explicit in canonical session metadata and canonical events.
5. Delegated coding work should use isolated worktrees by default.
6. Approvals, queue state, cancel, and steer remain session-local unless explicitly elevated.
7. OTEL should reflect delegation through linked traces and canonical lineage metadata, not by replacing the session store.
8. Branch identity and child-session identity should be kept distinct at the type level whenever possible.

## Immediate Implication

This document does not force subagent execution to become the next implementation slice.

It does force one architectural conclusion:

- when Belltower eventually "spawns itself," it should do so as a child session with explicit lineage and optional worktree isolation
- not as a branch pretending to be an independent worker
