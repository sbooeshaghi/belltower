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

Belltower can now perform a bounded form of "spawning itself." That means:

- create a child session
- attach explicit lineage to the parent session, branch, and turn
- select a configured connection and model independently for the child
- run the child session with its own queue, approvals, controls, and execution timeline
- exchange typed durable messages with the parent
- return a typed result, progress update, or error to the parent

This keeps branching, delegation, and telemetry understandable instead of collapsing them into one opaque transcript.

## Current Implemented Contract

Extended-tool sessions expose four model-facing operations:

- `spawn_agent` creates and starts a child session for one bounded objective
- `send_agent_message` sends a typed message to a direct parent or child
- `list_agents` reads lineage, runtime status, and recent durable mailbox state
- `wait_agent` waits for mailbox activity for at most 30 seconds; it does not join, cancel, or hide another turn

The child connection and model are independent session settings. Omitting both
inherits the parent settings; providing them allows mixed-provider workflows
such as an OpenAI parent coordinating local Ollama and Anthropic children.

Cross-session communication is a typed mailbox, not transcript reconstruction.
Each logical message commits one `Sent` event in the source session and one
`Received` event in the destination session atomically. Messages have a kind,
`notify` or `wake` delivery mode, optional reply correlation, text, and artifact
references. `notify` makes evidence available to a later turn. `wake` also asks
an idle destination session to claim the oldest pending message and begin an
ordinary runtime-owned turn. It never interrupts active work.

This is intentionally bounded:

- messages are limited to direct parent-child edges
- maximum lineage depth is four and maximum descendants per lineage is eight
- model-facing spawn always requires human approval
- child sessions currently share the parent's project root, so the tool also
  requires explicit `allow_shared_workspace: true`
- higher-order scheduling, role systems, recursive workflow policy, and
  worktree lifecycle are not part of this slice

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

`bt-runtime` owns only the reusable session-graph primitives: lineage
validation, atomic mailbox delivery, message claim, context assembly, and turn
admission. `bt-server` adapts those primitives into model-facing tools and
starts execution. Higher-order workflow coordination should remain outside the
per-session runtime; if it lands, a dedicated composition layer such as
`bt-workflow` is a better fit than adding scheduling policy to `bt-runtime`.

## Filesystem and Worktree Model

For coding agents, the target safe default remains:

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

The current bounded implementation has not landed worktree lifecycle yet.
Therefore `spawn_agent` treats shared-workspace execution as an explicit unsafe
mode: it requires both human approval and `allow_shared_workspace: true`. This
guard is not a substitute for worktree isolation; it keeps the current behavior
truthful until isolation has one clear owner and lifecycle.

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
- `session.related_message.recorded`
- `session.related_message.resolved`

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
either all commit or all roll back. Model-facing spawn commits the paired
initial-objective mailbox events in that same transaction. The parent turn id
is causal provenance and does not grant permission to append new live evidence
to a completed turn.

Related-session delivery follows the same rule. The source and destination
event copies commit together or not at all. A `wake` claim atomically appends
`session.related_message.resolved` plus `turn.started { source:
related_session_message }` before provider or tool execution. The destination's
current settings revision is captured at claim time; a concurrent settings
change retries admission instead of silently stranding the wake. When both
session logs are present in one store, the derived sender and receiver mailbox
rows expose the same claimed or dropped status. Server startup claims pending
idle-session wakes through ordinary admitted turns. A previously claimed
interrupted turn is terminalized rather than retried, because tool side effects
may already have occurred; runtime reopening repairs one durable typed error to
the sender when the interruption terminal event committed before that reply.

## Spawn Contract

Spawning a child session requires a structured contract, not just a free-form prompt.

The implemented model-facing contract includes:

- objective
- optional display name
- configured connection and model selection
- explicit shared-workspace acknowledgement
- parent session, branch, and active-turn provenance

The fuller workflow contract may later add:

- parent handoff summary
- working directory or worktree
- instruction bundle
- tool allowlist or policy
- MCP scope
- budget limits
- expected return artifact

The current return is a typed `result`, `progress`, or `error` mailbox message
with optional artifact references and reply correlation. A richer return
artifact should remain structured rather than becoming an untyped transcript
convention.

Reply settlement is a turn-run invariant, not a dispatch-path courtesy: at the
end of every turn run on a child session, each claimed `instruction`/`question`
message without a terminal reply receives one derived from the outcome. A
child that pauses for approval and is resumed by the operator therefore still
reports its eventual result. Terminal replies (`result`/`error`) are sent with
`wake` delivery so an idle parent runs a turn to consume them; waking cannot
ping-pong because a woken turn claims a `result`/`error` message, which never
creates a reply obligation. A server-side wake pump subscribed to the event
bus dispatches any `wake` message whose destination was idle when it arrived;
claims are atomic, so racing dispatchers resolve to one turn.

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

Auto-approval is the explicit policy form of inheritance: a session may run in
auto-approval mode (`approval_mode: "auto"` at create time, or per spawn with
`inherit`/`auto`/`prompt`, where `inherit` copies the parent's mode). In auto
mode every approval resolves as a durable `Policy { rule:
"session_auto_approve" }` decision — recorded, session-scoped, and auditable.
The mode flag itself is process-local and fail-safe: a restart falls back to
prompting. Spawn approval is how the operator consents to a child's mode: the
`approval_mode` argument is visible in the spawn arguments they approve, and a
parent already in auto mode delegates within the depth and descendant caps.

This matters because autonomous work should remain auditable.
If a child session used a dangerous tool, the operator must be able to answer which session approved it, under what scope, and why.

## UX Model

The operator and model surfaces distinguish:

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
- model-facing `list_agents` and `wait_agent`

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

The rollout required canonical events for:

- spawn request
- child session creation
- handoff summary
- result import

Status: complete for spawn, handoff, typed mailbox delivery, and mailbox
resolution. The older result-import events remain available for explicit
workflow import/rejection; live child turns normally return through typed
related-session messages.

### Phase 3. Manual Child-Session Spawn

Allow an operator or parent session to create a child session intentionally, without yet making autonomous spawning the default.

This is the safest first execution slice because:

- the telemetry model is exercised
- lineage and inspection surfaces can be validated
- worktree policy can be tested

Status: complete for the deliberate operator path.

The current manual operator surface is `/spawn [--connection <id>] [--model <model-id>] <objective>`. It is an explicit authenticated operator action rather than an agent-tool approval flow. It creates a child session, links it to the parent session, branch, and latest branch turn by default, records canonical spawn and handoff events, and leaves the operator in the parent session so the child can be resumed explicitly. It does not autonomously execute the child in the shared project root.

If `--connection` or `--model` are omitted, runtime inheritance remains the source of truth: the child inherits the parent connection, and it inherits the parent model only when the connection is unchanged. This keeps model selection explicit when the operator intentionally assigns a child session to a different provider.

### Phase 4. Bounded Autonomous Child Sessions

Status: implemented as an explicitly bounded foundation.

`spawn_agent` is not a general workflow scheduler. It always requires approval,
uses direct parent-child lineage, applies depth and descendant limits, and
requires explicit consent to the current shared project root. Child approvals,
queues, cancel, steer, settings, and turn ownership remain session-local.
`send_agent_message`, `list_agents`, and `wait_agent` provide durable
coordination without hidden process-local mailboxes.

Before widening this into delegated coding at scale, Belltower still needs
isolated-worktree lifecycle, explicit budget/tool-policy inheritance, and a
separate owner for higher-order scheduling policy.

## Design Rules

Every implementation should preserve these rules:

1. A branch is not a subagent.
2. A subagent is a child session, not a hidden nested transcript.
3. Parent and child sessions must be independently replayable.
4. Parent-child relationships must be explicit in canonical session metadata and canonical events.
5. Delegated coding work should use isolated worktrees by default; until then,
   autonomous model-facing shared-workspace spawn must stay explicit and
   approval-gated. Manual spawn is itself an explicit operator action and does
   not autonomously execute the child.
6. Approvals, queue state, cancel, and steer remain session-local unless explicitly elevated.
7. OTEL should reflect delegation through linked traces and canonical lineage metadata, not by replacing the session store.
8. Branch identity and child-session identity should be kept distinct at the type level whenever possible.

## Immediate Implication

Belltower's implemented subagent foundation is intentionally smaller than a
general multi-agent framework. It proves the durable child-session, mixed-model,
bidirectional-message, and restart-safe execution seams. New role systems,
recursive delegation policy, or workflow schedulers must justify themselves
against real dogfooding before they enlarge this contract.
