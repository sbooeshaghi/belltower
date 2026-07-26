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

- `budget.configured` is the sole owner of the durable limit shape
- `budget.checkpoint` advances only restart-safe tokens/turns/time/cost counters;
  it never copies limits from a stale runtime read
- budget policy may change only while the session is idle. The store rejects a
  configuration update while a turn owns the active slot; an idle update that
  is already exhausted by persisted counters records its configuration and
  resulting cancellation atomically. A prior cancellation for another reason
  does not suppress the distinct `budget_exhausted` event
- checkpoint time accounting is anchored to durable turn events and prior
  checkpoint timestamps, not a process-local timer that resets on restart
- checkpoint counters cannot regress. Fresh cost accounting starts at a known
  zero; if a completion has no price, cumulative cost becomes unknown and a
  configured cost ceiling fails closed. The cutover migration normalizes a
  legacy null cost to zero only when every persisted usage counter is also
  zero; historical null cost remains unknown
- runtime enforcement reads that projection rather than reconstructing budget
  state from process-local memory
- direct user admission and queued/steered continuation claims evaluate the
  projected budget inside the same write transaction that would acquire
  active-turn ownership; exhausted work remains durable but cannot be claimed
- terminal execution and interrupted-turn recovery keep active ownership until
  the store atomically commits the checkpoint, any resulting cancellation,
  terminal evidence, and `turn.finished`; normal completion, preflight failure,
  resumed failure, sibling approval suspension, and interrupted-turn recovery
  all follow this ordering. The transaction first proves that the exact
  session/branch/turn still owns the active slot, so duplicate or stale live
  finish attempts append nothing
- non-terminal live tool request/result transitions and in-turn approval
  evidence append through an exact session/branch/turn ownership transaction.
  After another runtime recovers a turn, the stale worker cannot append those
  lifecycle events. Deferred operator approval instead starts a newly admitted
  resume turn before execution
- standalone budget checkpoints also require the exact active owner; an old or
  already-finished turn cannot mutate durable budget accounting
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
- each settings revision identifies one immutable, self-contained connection,
  model, and tool-mode snapshot; the store allocates the next revision inside
  the same write transaction that appends `session.settings.updated`, and replay
  may only reassert identical contents
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
transaction, runtime checks the current settings revision, session control
state, and budget eligibility, then does exactly one of the following:

- appends the user message and `turn.started`, acquiring active-turn ownership
- appends `session.queued_message.enqueued` and its audit event, preserving the
  settings revision active at enqueue time

The server transports this typed outcome; it does not inspect process-local
state and independently decide whether work is busy. Two concurrent clients
therefore cannot both start turns for the same session. Queue and steer
continuations likewise claim their exact durable event identifiers and append
their resumed `turn.started` transition atomically, and cannot acquire work
after the session budget is exhausted. Approval and pending-input continuations
use the same rule: the store identifies the exact original request by its
session, branch, turn, call id, tool name, and request sequence, validates it is
still pending, and commits the resolution plus resumed `turn.started` in one
transaction. Cancellation remains pending and wins over resume; transport never
approximates either decision with a precheck.

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
provider call. Both transitions prove that the exact session/branch/turn still
owns the active slot before appending, so an already-recovered stale runtime
cannot add tool lifecycle evidence. Persistence and provider context share the exact tool-call and
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
provider construction, model execution, or tool preflight. Direct, queued, and
steered admission return an opaque `AdmittedTurn` capability for that exact
durable boundary; the turn orchestrator cannot be entered with a caller-supplied
turn id or a boolean that claims the turn was already started, and the
capability cannot be rebound to a different session or branch. Approval and
pending-input resume produce the same capability only after their exact pending
request and resumed-turn ownership commit atomically. If later preflight fails,
runtime appends a turn-bound `session.error` followed by `turn.finished {
status: "failed" }`. This ordering keeps ingress atomic without leaving stale
active ownership.
Persisting enough settings metadata to admit work after a connection has been
removed from local configuration remains follow-on hardening rather than
current behavior.

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
- the runtime validates the exact pending request and persists the human
  decision with the resumed `turn.started` boundary in one transaction
- budget, cancellation, and active-owner predicates are evaluated in that same
  claim; a rejected claim does not consume the pending request

The agent loop should never block on server IO it does not own.

Settings mutation is also part of the turn contract now:

- the store allocates settings revisions transactionally and never mutates an
  existing revision to contain different connection, model, or tool settings;
  each settings event is a complete snapshot so replay never consults mutable
  session-row state
- every turn starts under an explicit `settings_revision_id`
- paused turns resume under the revision captured by their original `turn.started`
- queued follow-up input runs under the revision captured when it was enqueued, not whatever the session row says later

## Compaction Ownership

Compaction ownership needs to stay split cleanly between `bt-runtime` and `bt-context`, not drift into `bt-agent`.

The clean model is:

- `bt-context` owns prompt shaping, token budgeting, and the compaction
  policy (trigger math, retention selection, summary composition); it is
  pure — the runtime supplies provider-observed usage and previous-summary
  chain state as inputs
- the runtime assembles the prompt-ready history for the next turn
- compaction fires proactively when provider-observed context tokens (the
  last real `completion.finished` usage total on the branch plus a bytes/4
  estimate of messages appended since) exceed
  `context.compaction_trigger_fraction` (default 0.9) of the model window;
  the whole-history estimate remains the hard fit guarantee and the
  fallback when no prior completion exists. The check runs in the
  orchestrator preflight for the initial turn and for every continuation
  dispatch. A model downshift is handled by the same pre-turn detection:
  usage observed under the old model is compared against the new model's
  window at the next preflight, so shrinking the window forces a
  compaction before the next completion (the sync settings-change path has
  no provider seam, so no separate downshift hook exists)
- retention is a sliding window on the canonical substrate: pinned system
  prefix, then older real user messages verbatim (newest-first within
  `context.compaction_user_message_budget_tokens`, default 20k), then the
  summary as a system message, then the recent tail verbatim (all roles).
  Cuts land only at message-unit boundaries: a tool_use and its
  tool_result — including approval-paused calls with interposed messages —
  are one indivisible unit and are never split
- the summary is model-written: the orchestrator preflight runs a
  summarization completion via the normal provider seam (same connection;
  model from `context.summarizer_model`, defaulting to the turn's model)
  over the dropped messages plus any previous summary, recorded
  canonically as `completion.requested`/`completion.chunk`/
  `completion.finished` under the reserved context-maintenance
  `llm_call_ordinal` 0 so it is auditable and cost-accounted. Any
  summarizer failure falls back to the deterministic digest — compaction
  never fails a turn. Summaries are always plaintext
- composition is a chain, not a stack: the previous summary (from the last
  `context.compacted` event, plus any marker-prefixed summary message
  found in history) is folded into the new summarization input and never
  retained alongside the new summary. Manual `/compact` keeps working on
  the synchronous path with the deterministic digest
- provider/auth/model preflight happens before runtime records model-visible
  context mutations such as `context.compacted`; a provider path that cannot
  execute records a durable failure instead of leaving a successful compaction
  fact behind
- the agent receives a ready-to-send request rather than discovering context-window failure mid-turn

Compaction must remain a canonical event and record:

- a stable compaction id
- the window chain: window number plus previous/first compaction ids on the branch
- whether compaction was forced or token-budget driven
- whether the compaction happened as manual operator work or as pre-turn context preparation
- completed or failed status
- provider and model for the prepared request
- the context boundary sequence id used for the prepared request
- the synthetic summary message id
- the first kept message id, branch id, and sequence id — message-id refs
  are mandatory (seq ids are stripped from portable bundles)
- compaction latency
- what was dropped or summarized
- the replacement summary (and, in `reason`, whether it was model-written
  or the digest fallback)
- token/message accounting before and after
- preserved file-read and file-modified metadata

When compaction succeeds, `bt-context` constructs the synthetic system summary
message (its text begins with the stable `[compaction summary]` marker) and
returns its message id. `bt-runtime` assigns the durable `compaction_id`,
derives the window chain from the branch's previous `context.compacted`
event, records `context.compacted`, and records a model-visible compaction
attachment in the following context manifest. `bt-agent` only sees the
prepared provider request; it does not decide or persist compaction.

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

## Related Sessions And Subagents

The current subagent foundation preserves the same ownership split as ordinary
turns:

- `bt-session` owns parent-child metadata, atomic paired mailbox events, message
  status projection, FIFO wake claims, and restart reconstruction
- `bt-runtime` owns lineage validation, spawn admission, message delivery and
  claim APIs, model-visible context assembly, destination settings resolution,
  and post-turn continuation ordering
- `bt-server` owns only the model-facing adapters and asynchronous execution
  host for `spawn_agent`, `send_agent_message`, `list_agents`, and `wait_agent`
- `bt-agent` remains unaware of session graphs and continues to operate on an
  ordinary typed turn request

Related-session messages do not become transcript messages. Received messages
are projected into model context with explicit source provenance only after
delivery or claim. A `wake` message starts work through the same admitted-turn
path as other continuations, and queued wakes are considered after cancel and
steer but before ordinary queued user follow-ups. `notify` messages remain
durable context for a later turn.

Child sessions have independent settings revisions, approvals, queues, cancel,
steer, and budgets. A child may use a different configured connection and model
from its parent. The runtime constrains model-facing spawn to depth four and
eight descendants per lineage; higher-order scheduling policy remains outside
this crate.

Recovery is deliberately conservative. Server startup claims one pending wake
per idle destination through the ordinary related-message admission path. If
the process restarts after a related message was claimed, runtime terminalizes
the interrupted turn and emits one typed error notification to the sender
instead of retrying possible tool side effects. Runtime reopening also repairs
that notification when the interruption terminal event committed before the
paired reply; an existing typed error reply suppresses duplication.

## Runtime Non-Goals

`bt-runtime` should stay focused on composition.

These concerns should be owned elsewhere when they land:

- higher-order workflow scheduling, role policy, or recursive delegation policy
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
