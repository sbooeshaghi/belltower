# Event Taxonomy and Write Path

This document defines the most important execution contract in Belltower:

- what the canonical event stream is
- how events move from runtime work into durable storage and live clients
- which crate boundaries are allowed to participate in that path

Read it with:

- [`./overview.md`](./overview.md)
- [`../subsystems/session-runtime-and-agent.md`](../subsystems/session-runtime-and-agent.md)
- [`../subsystems/telemetry-and-exports.md`](../subsystems/telemetry-and-exports.md)
- [`../planning/target-implementation-goals.md`](../planning/target-implementation-goals.md)

## Why This Matters

Belltower's moat is not "has a chat UI" or "can call tools."
It is that the canonical session record is durable, inspectable, branch-aware, and exportable.

That makes two things load-bearing:

1. the event taxonomy
2. the write path ordering

If either becomes informal, the architecture becomes easy to bypass accidentally.

## Canonical Write Path

Every canonical event must follow this path:

1. construct a typed `EventEnvelope`
2. commit it to the SQLite session store and assign `seq_id`
3. fan out the committed envelope to post-commit projections:
   - SSE/UI broadcast
   - `tracing` mirror
   - export/index refresh work

The important rule is not whether SSE or the tracing mirror happens first after commit.
The important rule is:

- both are driven by the committed envelope
- neither is allowed to invent canonical events on its own
- neither is allowed to become a hidden alternate source of truth

### Sacred Ordering Rules

These rules should be treated as invariants:

- no canonical session event is broadcast before SQLite commit succeeds
- no canonical session event is mirrored into `tracing` before SQLite commit succeeds
- SSE is driven by post-commit event fan-out, not by the tracing mirror
- pure runtime instrumentation spans may exist without a canonical event, but they must stay clearly separate from canonical session recording

### Allowed Exception

The only work that may exist purely in `tracing` is runtime instrumentation such as:

- `session`
- `turn`
- `llm_call`
- `tool_execution`

These spans exist to provide timing, nesting, and OTEL interop.
They are not a replacement for the canonical event log.

## The Event Taxonomy Is the Real Spec

The prose docs are explanatory.
The actual cross-crate contract is the canonical set of event kinds represented by `bt-core::EventPayload` and its stable kind strings.

Today the canonical families are:

### Session and Branch

- `session.started`
- `session.settings.updated`
- `session.ended`
- `branch.created`
- `branch.activated`
- `branch.summarized`

`session.started` marks the first canonical activity recorded for a session.
The durable session and default-branch rows exist before that first event so an
empty session can be addressed without inventing synthetic startup traffic.

### Messages and Turns

- `message.appended`
- `turn.started`
- `turn.instructions.recorded`
- `turn.context_manifest.recorded`
- `completion.requested`
- `completion.chunk`
- `completion.finished`
- `turn.finished`

`message.appended` carries the authoritative message content and role today.
If Belltower later splits this into more specialized message event kinds, the mapping must remain one-to-one and replay-safe.
`turn.instructions.recorded` captures the resolved core prompt, provider overlay, markdown instructions, and rendered system prompt for a turn so historical turns can be reconstructed without rereading mutable files from disk.
`turn.context_manifest.recorded` captures the model-visible manifest for each
provider call before `completion.requested`: branch id, context boundary
sequence, message refs with source branch/sequence when available, tool refs,
explicit attachment refs, prompt presence/counts, model, provider, settings
revision, and compaction status. It records what was included without
duplicating full prompt or message bodies already stored in canonical events.
The session store projects these records into `context_manifest_projection` so
inspection and export consumers do not need to reconstruct provider-call
boundaries from the raw event stream.
Runtime recomputes the message source references immediately before every
provider call, including same-turn continuations after tools that do not pause
for approval. A continuation therefore describes the exact committed context it
uses rather than inheriting a pre-tool manifest snapshot.

### Tools and Approvals

- `tool.operation.recorded`
- `tool.call.requested`
- `tool.approval.requested`
- `tool.approval.resolved`
- `tool.execution.finished`

`tool.operation.recorded` is the durable envelope for tool-operation metadata such as initiator, risk/read-only/execution-mode classification, and artifact references. The call and result events remain the canonical argument/result events; operation metadata exists so built-in tools, MCP tools, and human-invoked operations can converge on the same telemetry shape without overloading `tool.call.requested`.

Every `tool.call.requested` must have exactly one terminal
`tool.execution.finished`, including denied, cancelled, timed-out, aborted, and
failed execution. For agent-invoked tools, runtime atomically commits operation
metadata, the request, and its canonical assistant tool-call message before
execution. It atomically commits the terminal result and canonical tool-result
message before model re-entry. Those are the exact message identities later
referenced by the provider-call context manifest; runtime does not persist one
copy while passing a separately constructed copy to the model. For
human-invoked operator tools, runtime
atomically commits operation metadata and the request before execution, then
atomically commits the terminal result and `operator.command.recorded` audit
event. Operator tools do not fabricate assistant tool-call or tool-result
messages because their output is not model transcript history. Persistence
failure stops execution or provider re-entry at the relevant boundary; closing
turn/error events describe a stopped agent transition without allowing later
provider work to observe uncommitted tool evidence. If an operator terminal
batch encounters a recoverable append failure after execution, runtime retries
as a terminal/audit/error recovery batch so the durable request is not left
permanently open.

`tool.approval.requested` carries optional `ApprovalRequestSnapshot` evidence:
request fingerprint, argument hash, redacted argument preview, approval
requirement, tool metadata, initiator, surface, and optional policy/registry
identifiers. `tool.approval.resolved` carries optional `ApprovalResolution`
evidence tying the final decision back to that fingerprint. This preserves the
request-time policy facts separately from the later decision while keeping
`tool.call.requested` as the canonical executable argument source.
One-shot approval is a consumable authorization bound to session, call
identifier, and request fingerprint. It unlocks the suspended request exactly
once and is removed from runtime approval state when consumed; a later request
never inherits it, even when a provider repeats the same call identifier and
arguments. Projections and inspection similarly treat the newest
`tool.call.requested` as the current request instance, clearing stale terminal
and approval state until matching newer events arrive.
Reusable Session and Always approval scopes are reconstructed at startup by
replaying canonical `tool.approval.resolved` events in sequence order. The
mutable approval projection is an inspection cache, not the rehydration source.

### Runtime Control and Maintenance

- `context.compacted`
- `budget.configured`
- `budget.checkpoint`
- `session.queued_message.enqueued`
- `session.queued_message.resolved`
- `session.cancelled`
- `session.cancel.cleared`
- `session.steered`
- `session.steers.resolved`
- `operator.command.recorded`
- `raw_chunk.persisted`

The control-plane rule is now explicit:

- queued follow-up input is canonical runtime state, not a server-local queue
- cancellation is a durable request that stays pending until runtime clears it
- steer messages are durable queued control inputs that runtime later applies or drops
- `session.settings.updated` advances a durable `settings_revision_id`, and runtime control events capture the revision they should later run under
- `session.queued_message.enqueued` and `session.steered` capture a settings revision, and runtime resolves the matching historical settings snapshot before it starts the follow-up turn
- `turn.started` records the settings revision actually used for that turn, so paused turns can resume under their original model/connection
- `operator.command.recorded` is audit/output context only, not the source of truth for queue or control state
- `budget.configured` carries the canonical session autonomy limits, while `budget.checkpoint` carries the restart-safe used-so-far counters that runtime later enforces against
- budget checkpoint elapsed-time accounting is derived from durable turn and
  checkpoint timestamps, not a process-local timer
- budget exhaustion is recorded as `session.cancelled { reason:
  "budget_exhausted" }`, and that durable control state is what later
  inspection surfaces read
- `context.compacted` records a stable `compaction_id`, trigger, phase, status,
  provider/model, context boundary, summary message reference, first-kept
  message/source reference when available, latency, and before/after accounting.
  Context manifests use the same `compaction_id` in a model-visible compaction
  attachment, so inspection/export can prove which synthetic summary replaced
  dropped context without reverse-engineering the prompt text.

### Event Taxonomy Rule

Adding, removing, or repurposing one of these kinds is a cross-crate API change.
It affects:

- the SQLite store and projections
- SSE envelope consumers
- TUI rendering
- protocol DTOs
- export formatters
- OTEL/OpenInference mapping

That means event taxonomy changes deserve the same scrutiny as protocol changes.

## Turn Outcomes and Suspension

The agent loop must remain pure, but runtime control still needs explicit turn boundaries.

The runtime-facing turn contract should behave like a typed outcome:

- complete
- approval required
- cancelled
- budget exhausted

Today Belltower may represent that with structured result fields rather than a single enum in every path.
That is acceptable only if the semantics remain explicit and testable.

The important rule is:

- approval pause and resume are not hidden blocking IO inside `bt-agent`
- the runtime persists the approval request with request-time evidence
- the turn suspends cleanly
- the runtime later persists the approval decision and bootstraps canonical
  resumed-turn state before the server execution path continues the turn

## Read Model and Replay

Fresh transcript/session reconstruction should come from projection-backed reads, not from replaying the entire raw event stream in the UI.

The intended model is:

- resume from session and branch inspection surfaces
- load transcript windows from projection-backed message views
- use SSE with `Last-Event-ID` to catch up from the true session tail
- use raw event replay for reconnect/catch-up and offline analysis, not as the primary UI bootstrap path

This distinction becomes mandatory for long sessions with thousands of messages, tool calls, and raw chunks.

## Raw Chunks Are Part of the Contract

Raw stream durability is only useful if it is queryable.
The raw chunk model should support, at minimum:

- all raw chunks for a given turn on a given branch
- all raw chunks for a particular LLM call within a session
- comparison between the structured event projection and the raw provider stream for the same turn

That implies durable identifiers and indexes that connect raw chunks back to:

- `session_id`
- `branch_id`
- `turn_id`
- provider/model or call ordinal
- chunk order

## Failure Semantics

SQLite durability is not optional in Belltower's canonical path.

If the canonical store is unavailable because of:

- migration failure
- disk full
- lock failure
- other storage errors

then canonical turn progression should stop.
Belltower should not silently degrade into a hidden non-durable mode for normal interactive use.

## Crate Boundary Implications

These rules follow directly from the write path:

- `bt-agent` emits structured turn output; it does not write to SQLite
- `bt-runtime` composes and commits canonical events
- `bt-server` exposes committed state over protocol surfaces
- `bt-tui` renders committed or protocol-derived state, not hidden local-only truth
- `bt-otel` maps committed Belltower-owned fields into OTEL/OpenInference views

The store is canonical.
Everything else is a projection or a consumer.
