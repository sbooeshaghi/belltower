# Session, Runtime, and Agent

This document covers the execution core of Belltower:

- `bt-session`
- `bt-runtime`
- `bt-agent`

The closest case-study analogue is `@mariozechner/pi-agent-core`, plus `pi`'s session/tree docs and Hermes's session storage architecture.

Read it with:

- [`../architecture/event-taxonomy-and-write-path.md`](../architecture/event-taxonomy-and-write-path.md)

## Purpose

This layer makes Belltower an agent harness rather than just a model client.

It is responsible for:

- durable session history
- runtime composition and control
- the pure turn loop

## Crate Responsibilities

### `bt-session`

Owns:

- SQLite schema and migrations
- append-only event writes
- raw chunk durability
- branch tracking
- projection-backed reads
- replay helpers

### `bt-runtime`

Owns:

- session services
- branch services
- approval state
- connection registry
- instruction resolution
- control queues for cancel and steer
- persisted session budget state and restart-safe budget accounting
- provider/tool/session composition
- context preparation orchestration
- canonical compaction decisions and persistence

### `bt-agent`

Owns:

- turn execution
- streaming accumulation
- tool-call re-entry
- retries
- finish-state handling
- per-turn budget/completion semantics inside the typed turn loop

It must remain storage-agnostic and server-agnostic.
It should not depend on `bt-session`, `bt-runtime`, `bt-server`, or `bt-auth`.
That dependency closure is worth enforcing mechanically, not just by convention.

The agent loop may emit runtime-facing turn results, but it must not write directly to SQLite or any other canonical store.

## Storage Model

The canonical record is the Belltower-owned SQLite store.

At minimum it includes:

- `sessions`
- `branches`
- `events`
- `raw_chunks`
- projections for messages, approvals, tool runs, context manifests, branch
  heads, costs, and session budgets

`active_turn_projection` is a rebuildable store-owned coordination read model.
It enforces at most one active turn per session while keeping
`turn.started`/`turn.finished` as the canonical historical record. A matching
`turn.finished` releases ownership; a stale finish for an older turn cannot
clear a newer active turn.

The budget projection is the canonical read model for session autonomy limits and
used-so-far counters:

- `budget.configured` sets or updates the durable limit shape
- `budget.checkpoint` advances the restart-safe tokens/turns/time/cost counters
- checkpoint time accounting is anchored to durable turn events and prior
  checkpoint timestamps, not a process-local timer that resets on restart
- runtime enforcement reads that projection rather than reconstructing budget
  state from process-local memory
- `session.cancelled { reason: "budget_exhausted" }` is the canonical stop
  marker when a configured ceiling is reached at a turn boundary

The durable session and branch rows are store-owned records.
`session.started` is emitted on first canonical activity, not as a synthetic
session-row creation event.

Important design rules:

- writes stay serialized
- reads come from projections where possible
- replay comes from the canonical stream
- branching stays explicit and replayable
- canonical events are committed before any SSE or tracing projection is emitted
- if the canonical store is unavailable, canonical turn progression stops rather than silently degrading into a non-durable mode

Model-visible turn input is also store-owned. Runtime context preparation loads
branch-aware message records from `bt-session`, preserving the source branch and
`seq_id` for each included message. `turn.context_manifest.recorded` is then
projected into `context_manifest_projection` so inspection, export, and future
memory/context features can compare the exact provider-call boundary without
rescanning raw events or inferring prompt contents from the TUI.

## Event Sequence

The most useful thing copied from `pi-agent-core` is not the code, but the explicit event sequence.

User ingress is one atomic store transition. Under one SQLite write
transaction, runtime checks the current settings revision and session control
state, then does exactly one of the following:

- appends the user message and `turn.started`, acquiring active-turn ownership
- appends `session.queued_message.enqueued` and its audit event, preserving the
  settings revision active at enqueue time

The server transports this typed outcome; it does not inspect process-local
state and independently decide whether work is busy. Two concurrent clients
therefore cannot both start turns for the same session. Queue and steer
continuations likewise claim their exact durable event identifiers and append
their resumed `turn.started` transition atomically.

For a normal prompt:

```text
user message
-> session event append
-> turn.started
-> turn.instructions.recorded
-> assistant turn starts
-> streamed assistant chunks
-> assistant message finalized
-> turn.finished
```

For a tool-using turn:

```text
user message
-> turn.started
-> turn.instructions.recorded
-> assistant streamed response
-> assistant tool call finalized
-> operation metadata, tool request, and assistant tool-call message persisted atomically
-> approval or policy decision
-> tool execution
-> terminal tool result and tool message persisted atomically
-> assistant continuation turn
-> completion
-> turn.finished
```

The tool request and terminal result boundaries are synchronous. The operation
metadata, request event, and corresponding model-visible assistant tool-call
message are one runtime-owned atomic transition. Failure to persist that
transition prevents execution. The terminal event and corresponding
model-visible tool-result message form a second atomic transition; failure to
persist it prevents the result from entering context or triggering another
provider call. Persistence and provider context share the exact tool-call and
tool-result message identities; the following context manifest points back to
those canonical message rows and sequence identifiers. Final turn-result
persistence filters lifecycle-owned tool-call and tool-result messages while
persisting the remaining transcript messages, so the lifecycle observer cannot
create duplicate rows.
Runtime refreshes source branch and sequence references immediately before each
provider call, including same-turn continuations after safe tools that require
no approval. It does not reuse the pre-tool manifest boundary for later calls.

Human-invoked operator tools use the same canonical operation, request, and
terminal lifecycle without pretending to be assistant transcript output. Their
admission pair is `tool.operation.recorded` plus `tool.call.requested`; their
terminal pair is `tool.execution.finished` plus
`operator.command.recorded`. Both pairs are atomic. This keeps inspection and
telemetry uniform while preserving the distinction between model context and
operator audit history. If the initial terminal batch fails after execution,
runtime writes a terminal/command/session-error recovery batch before returning
the original persistence failure, preventing a recoverable append fault from
leaving the operation indefinitely requested.

Request identity is more specific than a provider-supplied call identifier.
One-shot approvals are consumable authorizations bound to session, call
identifier, and request fingerprint. They unlock only the suspended request
that received the decision; a later request cannot reuse that decision even if
the provider repeats the same call identifier and arguments. If a call
identifier is reused, projections and inspection resolve it to the latest
request instance and clear stale result or approval state until that instance
receives its own terminal and decision events.
Session and Always approval scopes survive restart because runtime replays the
canonical `tool.approval.resolved` stream in sequence order. It does not treat
the mutable approval projection as durable policy state.

For a paused approval:

```text
assistant requests tool
-> approval request persisted
-> turn.finished records awaiting_approval and releases active ownership
-> human approves or denies
-> approval event persisted
-> runtime atomically starts a resumed turn or terminates the request accordingly
```

Pending user input follows the same boundary with `awaiting_input`. A paused
turn is historical, not secretly active: continuation creates a new
`turn.started` carrying the original settings revision and resume provenance.

These sequences now need exact turn identifiers and event names, not just implied phases. The runtime should be able to answer:

- which events belong to which turn
- which turn was waiting on approval
- which turn resumed after approval
- which turn performed a given tool call or compaction step

The event taxonomy is therefore the real coordination surface.
At the prose level, the sequences above should map one-to-one onto canonical event kinds such as:

- `message.appended`
- `turn.started`
- `completion.requested`
- `completion.chunk`
- `tool.call.requested`
- `tool.approval.requested`
- `tool.approval.resolved`
- `tool.execution.finished`
- `completion.finished`
- `turn.finished`
- `context.compacted`
- `session.cancelled`
- `session.steered`

Runtime startup treats any `turn.started` without a matching `turn.finished`
as an interrupted turn and appends canonical failure events before exposing the
session as idle again. Resumed approval/input turns retain the
`interrupted_after_resume` finish reason; ordinary active turns use
`interrupted_after_restart`. This keeps recovery visible in session execution,
telemetry, and export rather than leaving stale live state in memory.
If execution may have begun but no terminal transition exists, recovery appends
a non-retryable `tool_outcome_unknown_after_restart` result for that specific
request instance. It never retries a potentially side-effecting tool
automatically. A durably denied request receives a deterministic denied result;
calls still waiting for an approval decision or user input remain explicitly
pending rather than being misclassified as interrupted execution.
The same unknown, non-retryable terminalization applies to interrupted unbound
human operator requests. Recovery preserves their operator origin and never
silently promotes them into agent transcript context or retries them.

After resolving the configured connection identity and model label required by
`turn.started`, direct user admission records that boundary before auth,
provider construction, model execution, or tool preflight. If a later
preflight fails, runtime appends a turn-bound `session.error` followed by
`turn.finished { status: "failed" }`. This ordering keeps ingress atomic
without leaving stale active ownership. Persisting enough settings metadata to
admit work after a connection has been removed from local configuration remains
follow-on hardening rather than current behavior.

## Turn Contract

Approval pause/resume is a coroutine problem, not a UI trick.

The runtime-facing turn boundary should behave like an explicit typed outcome, for example:

- complete
- approval required
- cancelled
- budget exhausted

Today Belltower may represent that with structured turn results rather than a single enum in every path.
That is acceptable only if the semantics remain explicit:

- the runtime persists the approval request
- the turn suspends cleanly
- the human decision is persisted as a canonical event
- the runtime bootstraps canonical resumed-turn state from that decision before
  the server execution path continues the turn

The agent loop should never block on server IO it does not own.

Settings mutation is also part of the turn contract now:

- every turn starts under an explicit `settings_revision_id`
- paused turns resume under the revision captured by their original `turn.started`
- queued follow-up input runs under the revision captured when it was enqueued, not whatever the session row says later

## Compaction Ownership

Compaction ownership needs to stay split cleanly between `bt-runtime` and `bt-context`, not drift into `bt-agent`.

The clean model is:

- `bt-context` owns prompt shaping, token budgeting, and compaction algorithms
- the runtime assembles the prompt-ready history for the next turn
- if that history does not fit the effective budget, the runtime invokes compaction before calling the agent
- provider/auth/model preflight happens before runtime records model-visible
  context mutations such as `context.compacted`; a provider path that cannot
  execute records a durable failure instead of leaving a successful compaction
  fact behind
- the agent receives a ready-to-send request rather than discovering context-window failure mid-turn

Compaction must remain a canonical event and record:

- a stable compaction id
- whether compaction was forced or token-budget driven
- whether the compaction happened as manual operator work or as pre-turn context preparation
- completed or failed status
- provider and model for the prepared request
- the context boundary sequence id used for the prepared request
- the synthetic summary message id
- the first kept message id, branch id, and sequence id when known
- compaction latency
- what was dropped or summarized
- the replacement summary
- token/message accounting before and after
- preserved file-read and file-modified metadata

When compaction succeeds, `bt-context` constructs the synthetic system summary
message and returns its message id. `bt-runtime` assigns the durable
`compaction_id`, records `context.compacted`, and records a model-visible
compaction attachment in the following context manifest. `bt-agent` only sees
the prepared provider request; it does not decide or persist compaction.

## Branching Model

Branching is first-class.

That means:

- branch heads are explicit
- replay is branch-aware
- switching branches is not a UI trick
- rewind is modeled as an append-only branch fork from a selected parent event,
  not as mutation of prior events
- summaries may be attached to branches when leaving active work

This is one of the strongest `pi` patterns worth preserving in spirit.

## Session History Search

Session history search is a store-backed inspection surface, not model memory.

The owning seam is:

- `bt-session` searches canonical stored events and returns stable references
- `bt-runtime` exposes the query as a read-only runtime facade
- `bt-server` and clients transport the typed response
- model-facing access uses the normal read-only `session_search` tool path

The default search scope is the active branch lineage so the model searches the
same durable history that would be considered relevant to the current branch.
Cross-branch search is explicit. Search results must identify the session,
branch, turn, event sequence, event id, and tool call when applicable, so later
inspection, export, replay, and operator review all refer to the same canonical
objects.

This surface should remain exact-text search over historical session events.
If Belltower later adds semantic memory or cross-session retrieval, that belongs
behind a distinct memory subsystem and should not be smuggled into this runtime
inspection path.

## Control Model

The runtime must own:

- cancel
- steer
- session-level budget state
- restart-safe budget accounting
- budget-driven control-plane outcomes such as cancellation and inspection state

These are runtime/session concerns, not TUI-local features.

The clean budget split is:

- `bt-agent` stays pure and emits the typed turn outputs that budget
  checkpointing consumes
- `bt-runtime` owns persisted budget configuration, restart-safe
  accounting, checkpoint emission, and session/control-plane consequences
- `bt-server` only surfaces that runtime-owned behavior through the
  transport boundary

Runtime startup recovery should stay store-query driven. Reopening the runtime
closes unfinished resumed turns from durable turn boundary events
(`turn.started` / `turn.finished`) rather than reconstructing full session
inspection for every historical session.

## Runtime Non-Goals

`bt-runtime` should stay focused on composition.

These concerns should be owned elsewhere when they land:

- workflow or subagent orchestration
- worktree lifecycle management
- export formatting
- provider-specific protocol behavior

If provider-specific `match` arms start accumulating in `bt-runtime`, the abstraction is leaking.

## Failure And Error Model

Canonical errors should be durable session events, but they do not all mean the same thing.
The architecture should continue to distinguish at least:

- provider errors
- tool errors
- protocol/reconnect errors
- storage/runtime errors

That distinction will matter for retry behavior, TUI rendering, and later workflow inspection.

Canonical `session.error` events are part of session history and export/telemetry. They should be
available through runtime and model-facing inspection surfaces such as `inspect({ query: "errors"
})`. TUI-local diagnostics may still exist for purely local failures, but those should not be
confused with canonical session errors.

## Design Commitments

- the runtime/store remain the source of truth for session state
- the agent loop must stay reusable and pure
- branches and replay must work without scanning serialized enum blobs
- autonomous behavior must remain interruptible

## Human Checkpoints

1. create a session and submit a prompt
2. reconnect to the same session and replay events
3. branch from an earlier point and continue on both branches
4. trigger a tool call and verify pause/resume behavior
5. inspect raw chunks plus finalized messages for the same turn

## Current Priorities

The next important improvements for this layer are:

- typed failure taxonomy that distinguishes provider, tool, protocol, auth, and storage/runtime failures
- workflow-oriented inspection and canonical delegation events that stay outside the per-session runtime crate
- making the runtime-facing turn outcome contract more explicit and more directly testable
- replay- and dataset-ready query ergonomics over turns, branches, and related sessions
- continued compaction correctness and branch-aware context integrity under long-running sessions
