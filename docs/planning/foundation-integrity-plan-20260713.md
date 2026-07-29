# Foundation Integrity Plan

Date: 2026-07-13

Baseline: `34f57b0` (`Harden session bundles and TUI controls`)

This plan closes the gap between Belltower's strong architectural foundation
and the failure modes found during a repository-wide code, documentation, and
Codex comparison audit.

The objective is not to add features. It is to make the reusable chat-agent
envelope trustworthy under concurrency, interruption, restart, branching, and
portable replay.

Read this plan with:

- [`../../AGENTS.md`](../../AGENTS.md)
- [`../development/developer-guidelines.md`](../development/developer-guidelines.md)
- [`../development/source-of-truth-matrix.md`](../development/source-of-truth-matrix.md)
- [`../architecture/overview.md`](../architecture/overview.md)
- [`../architecture/event-taxonomy-and-write-path.md`](../architecture/event-taxonomy-and-write-path.md)
- [`../architecture/session-bundles-and-trace-sync.md`](../architecture/session-bundles-and-trace-sync.md)
- [`../subsystems/session-runtime-and-agent.md`](../subsystems/session-runtime-and-agent.md)
- [`../subsystems/tools-context-and-approvals.md`](../subsystems/tools-context-and-approvals.md)
- [`../subsystems/telemetry-and-exports.md`](../subsystems/telemetry-and-exports.md)
- [`../subsystems/launcher-and-tui.md`](../subsystems/launcher-and-tui.md)

## Review Conclusion

Belltower already has the right durable core:

- typed canonical events
- SQLite commit before SSE and tracing fanout
- raw provider output durability
- projection-backed inspection
- server-first protocol clients
- restart-safe queue and control records
- portable `session.bt` validation and import
- a storage-pure agent loop

The main remaining risk is that a few important transitions are not atomic at
the same boundary as their canonical evidence. A side effect can happen before
its result is committed, concurrent requests can claim the same work, and some
derived consumers reconstruct context independently of the canonical request
boundary.

Codex is useful as a reference for replacement-context checkpoints, terminal
tool lifecycles, strict configuration, recovery tests, and TUI streaming. Its
in-process event queue is not a replacement for Belltower's canonical store.

## Governing Decisions

1. The canonical event store remains the source of truth.
2. Every externally visible side effect has one terminal canonical outcome.
3. Session-local identifiers are keyed by session unless their type guarantees
   global uniqueness.
4. Claiming work and recording the claim happen atomically.
5. `bt-runtime` owns turn and control transitions; `bt-server` transports them.
6. `bt-agent` remains independent of storage, server, auth, and session crates.
7. Model-visible context is a durable, reconstructable request boundary.
8. Derived telemetry consumes canonical manifests instead of rebuilding truth.
9. Active development permits clean schema cutovers when canonical events can
   rebuild projections. Compatibility code must justify its ongoing cost.
10. Documentation changes in the same slice as behavior or ownership changes.

## Execution Model

Each slice lands independently with its focused tests and documentation.
Only one writer may edit a hotspot at a time:

- `bt-session` migration, append transaction, and projection code
- `bt-runtime` turn/control orchestration
- `bt-agent` turn-loop lifecycle
- `bt-server` route handlers and tool composition
- core event, context-manifest, and protocol shapes
- TUI stream and history state

Explorer and verifier agents may run in parallel. Writer agents receive
disjoint file ownership, start from the latest integrated commit, and must not
revert unrelated work. The integrator reviews the actual diff before merge.

## Phase 1: Canonical Integrity

These changes are small, local, and prerequisites for later work.

### 1.1 Scope tool and approval projection identity

**Owner:** `bt-session`

**Problem:** `call_id` is provider-controlled but is the sole primary key of
tool and approval projections. Equal IDs in different sessions overwrite one
another.

**Change:**

- key both projections by `(session_id, call_id)`
- update all upserts and reads to use the composite identity
- rebuild these derived projections from canonical events during the cutover
- add cross-session collision tests for tool and approval records

**Invariant:** one session cannot alter another session's projected state.

### 1.2 Require terminal tool outcomes

**Owners:** `bt-runtime`, then thin `bt-server` adoption

**Problem:** approved/resumed execution failures can record `turn.failed`
without `tool.execution.finished`, leaving the tool permanently requested.

**Change:** record a typed error `ToolResult` for every failed, denied,
cancelled, timed-out, or aborted requested tool before closing the turn.

**Invariant:** every `tool.execution.requested` has exactly one terminal result.

### 1.3 Reject unknown configuration keys

**Owner:** `bt-core`

**Problem:** misspelled TOML keys silently fall back to defaults.

**Change:** use strict ignored-path reporting with useful key paths and tests.

**Invariant:** accepted configuration is the configuration Belltower applies.

### 1.4 Remove misleading public and dependency surfaces

**Owners:** the crate that exposes each surface

**Change:**

- remove `BelltowerClient::try_from(ConnectionId)`, which ignores its input
- remove the unused direct `bt-agent` dependency from `bt-server`
- update repository metadata that still points at the former organization
- replace provider-local zero-price placeholders with one truthful pricing
  ownership contract

**Invariant:** public APIs and package metadata describe real behavior.

## Phase 2: Atomic Runtime Ownership

This phase establishes one canonical owner for executable work.

### 2.1 Atomically claim one active turn per session

**Owners:** `bt-session` transaction primitive, `bt-runtime` transition

**Problem:** two direct messages can both observe an idle session and start
interleaved turns.

**Change:** define a store transaction that either commits `turn.started` for
an idle session or returns the canonical queued outcome. All transports call
that runtime transition.

**Invariant:** a session has at most one executable active turn.

#### 2.1a Linearize settings and budget admission

**Owners:** `bt-session` transaction primitives, `bt-runtime` policy

**Status:** complete (2026-07-14)

**Problem:** settings revisions are currently incremented before the durable
write transaction, and the HTTP transport checks budget separately from turn
admission. Concurrent updates can reuse a revision, while a turn can race a
budget-exhaustion checkpoint.

**Change:** assign settings revisions in one store transaction and move the
budget predicate into runtime-owned admission/continuation claims. Keep
provider, auth, and tool construction outside SQLite.

**Invariant:** every revision identifies one immutable settings snapshot, and
exhausted sessions cannot acquire new executable work.

**Implementation evidence:** the store now allocates and appends settings
revisions under one `BEGIN IMMEDIATE` transaction, rejects conflicting replay
of an existing full settings snapshot, and evaluates budget eligibility in
direct, queued, and steer claims before acquiring active-turn ownership.
`budget.configured` remains the sole limit owner; counter-only checkpoints and
any resulting cancellation commit atomically against current limits while the
active-turn slot is still held. Terminal transitions commit their checkpoint,
optional cancellation, terminal evidence, and `turn.finished` together,
after atomically proving that the exact session/branch/turn still owns the
active slot, including interrupted-turn recovery. Duplicate or stale production
finish attempts are rejected without appending evidence. Checkpoint counters cannot regress; fresh
cost accounting begins at zero and unknown completion cost fails closed under a
cost ceiling. Budget policy updates are idle-boundary operations and atomically
record a distinct cancellation if persisted counters already exhaust the new
limits. Non-terminal live tool request/result transitions, in-turn approval
evidence, and standalone budget checkpoints also prove exact active-turn
ownership in their append transaction, preventing a recovered stale runtime
from extending the turn. Runtime issues an opaque admitted-turn capability bound to one session
and branch for every execution path.

#### 2.1b Recover idle durable continuations

**Owners:** `bt-session` claim primitives, `bt-runtime` dispatcher, `bt-server`
execution host

**Status:** complete (2026-07-14)

**Problem:** a crash after `turn.finished` but before post-turn dispatch can
leave queued or steered work durable but idle. Startup recovery terminalizes
interrupted turns but does not wake their pending tail.

**Change:** add one runtime claim operation for the next idle continuation.
Reuse `turn.started` as the ownership claim, preserve steer-before-queue and
queue FIFO policy, block on pending approval/input/cancel, and let the server
host execute the already-claimed turn. Do not add an in-memory queue or another
durable ownership table.

**Invariant:** durable pending work is either blocked by explicit canonical
control state or becomes executable exactly once after restart.

**Implementation evidence:** queue and steer projections retain their exact
source event identifiers. Runtime claims one eligible continuation under the
active-turn and budget predicates, appends its resolution plus `turn.started`
atomically, and startup dispatches that already-claimed capability. Restart,
FIFO, branch, cancel, and exactly-once scenarios are covered by runtime and
server tests.

### 2.2 Atomically claim approval and input resume

**Owners:** `bt-session`, `bt-runtime`

**Status:** complete (2026-07-14)

**Problem:** concurrent approval requests can both pass a pending-state read
and execute the same side effect. Cancellation can be cleared while resuming
pending work.

**Change:** claim pending approval/input in the same transaction that validates
its state. A durable cancel wins over resume until explicitly superseded by a
new operator operation.

**Invariant:** pending work is consumed once; cancelled work does not execute.

**Implementation evidence:** approval and input projections retain branch,
turn, and original request sequence. The store claim compares that full request
identity, evaluates budget, cancellation, and active ownership, and appends the
decision/result evidence plus resumed `turn.started` in one transaction. A
stale reused call id, concurrent owner, exhausted budget, or pending cancel
does not consume the request or clear cancellation. Runtime returns the same
opaque admitted-turn capability used by direct and queued work.

### 2.3 Make cancellation interrupt real work

**Owners:** `bt-runtime`, `bt-agent`, provider/tool adapters

**Partial status (2026-07-28):** branch attribution and active-turn ownership
are complete. Cancel/steer/operator-command/operator-shell requests now require
an explicit branch. Runtime validates active ownership and appends the control
event under one serialized store boundary; protocol, restart, branch-history,
shell-lifecycle, and TUI invocation-snapshot tests cover the cutover. Threading
live cancellation through every provider/tool adapter remains open.

The same event-attribution pass now covers session settings and budget policy:
their projections remain session-scoped, but request DTOs require the invoking
branch and canonical events never infer the default branch. Related-session
messages likewise require an explicit destination branch, with exact reverse-
edge reply enforcement in runtime and the atomic store transaction.

**Change:** thread a runtime cancellation capability through provider streams,
built-in tools, and MCP calls. Shell cancellation terminates the child process
group. MCP calls have bounded timeouts and truthful degraded state.

**Invariant:** cancel is an observable transition, not a post-completion hint.

### 2.4 Commit tool evidence at the side-effect boundary

**Owners:** `bt-agent` lifecycle interface, `bt-runtime` durable observer

**Problem:** the agent can execute a tool and use its in-memory result in a
later provider request before runtime commits the result.

**Change:** add a small typed lifecycle observer to the pure agent loop. Runtime
implements it by committing request/result evidence before the result can enter
the next provider request. The interface has no storage types.

**Invariant:** no provider call depends on an uncommitted tool result.

### 2.5 Move semantic composition out of transport

**Owner:** `bt-runtime`; `bt-server` becomes a caller

Move built-in/MCP/web tool composition and provider credential adaptation
behind a runtime-owned service interface after the lifecycle contract is
stable. Do not move HTTP parsing, auth middleware, or SSE fanout into runtime.

## Phase 3: Exact Reconstruction

### 3.1 Persist replacement-context compaction

**Owners:** `bt-core` shape, `bt-session` projection, `bt-runtime` transition,
`bt-context` consumption

Manual compaction must produce a durable checkpoint that replaces the covered
history during later context construction. The summary reference must resolve
to stored content. Branches created before the checkpoint remain unchanged.

**Invariant:** context built after restart is identical to context immediately
after compaction.

### 3.2 Make context manifests exact

**Owners:** `bt-context`, `bt-core`, `bt-session`

Record the exact normalized/truncated model-visible message representation or a
content-addressed reference to it, including normalization version and hashes.
Visible-character counts alone are insufficient.

**Invariant:** a provider request can be reconstructed byte-for-byte from
canonical session data and referenced blobs.

### 3.3 Make telemetry consume manifests

**Owner:** `bt-otel`

Build LLM span input attributes from the canonical context manifest and its
branch-scoped references. Remove the independent global trailing-message
reconstruction path.

### 3.4 Close raw-stream and search gaps

**Owner:** `bt-session`

- remove or internalize the public unpaired raw-chunk append path
- ensure portable export rejects rather than silently omits unpaired chunks
- define whether exact search includes completion chunks and raw payloads;
  expose separate scopes if searching raw provider bytes is too expensive

### 3.5 Enforce append-only remote ancestry

**Owner:** `bt-session` sync

In addition to compare-and-swap, prove that the proposed branch head descends
from the expected remote head. Divergent work must create a new branch/ref.

## Phase 4: Consumer and Operator Truth

### 4.1 Bound and unify readiness

**Owners:** `bt-readiness`, `bt-models`, providers

- apply one timeout to validation plus model discovery
- set HTTP client request timeouts
- avoid duplicate `/models` probes in one readiness operation
- report empty/malformed local inventory as degraded, not ready
- centralize local backend matching in `bt-readiness`

### 4.2 Complete the remote/headless contract

**Owners:** `belltower`, `bt-client`, `bt-protocol`

- accept explicit bearer authentication for remote headless use
- publish real OpenAPI schemas for public DTOs
- encode `SendMessageResponse` as its actual discriminated union

### 4.3 Make branch-safe TUI replay and reconnect truthful

**Owner:** `bt-tui`, with protocol changes only if branch filtering cannot be
expressed by the current stream

- prevent inactive-branch events from entering the active transcript
- reload branch history after activation
- expose reconnecting state
- render approval snapshots from canonical inspection
- restore failed question answers to the composer
- add reconnect/replay/branch tests

The committed/live history seam should converge on one offset-based collector.
Do not rewrite the TUI wholesale while correctness fixes are landing.

### 4.4 Validate tool inputs and bound MCP calls

**Owners:** `bt-tools` validation contract, `bt-mcp` adapter

Validate arguments against the declared schema before approval or execution.
Unknown MCP policy remains conservative. Timeouts and failures produce terminal
canonical tool outcomes.

### 4.5 Make delegated-session isolation truthful

**Owners:** architecture decision first, then runtime/protocol

Either implement explicit worktree identity and safe isolated defaults, or
change docs and UI to say child sessions share the project root. Approval reuse
must state whether `Always` is process-global, project-scoped, or principal-
scoped; it must not implicitly leak through a parent/child relation.

## Acceptance Gates

Every slice runs:

1. `cargo fmt --all`
2. `cargo check -p <affected-crate>`
3. `cargo test -p <affected-crate>`

Cross-crate shape or ownership changes also run:

1. `cargo check --workspace`
2. `cargo test --workspace`
3. `make acceptance`

Required focused scenarios:

- equal provider call IDs in two sessions remain isolated
- two simultaneous sends yield one dispatched and one queued operation
- two simultaneous approval decisions execute the tool once
- a stale resume for a reused call id cannot claim the newer request instance
- cancel while approval/input/provider/tool work is pending prevents execution
- every requested tool reaches one terminal outcome through restart
- compact, restart, and rebuild produce identical model-visible context
- OTLP input refs equal the canonical manifest on a branched compacted session
- remote sync rejects non-descendant head replacement
- TUI reconnect and branch activation do not duplicate or contaminate history
- readiness returns within its deadline for a hanging endpoint
- a failed child-session spawn leaves neither a partial child nor partial
  parent/child lineage events

## Deliberately Deferred

These may be valuable, but they do not solve a measured foundation failure:

- autonomous hierarchical scheduling
- new memory policies
- transport aliases and compatibility layers
- general hook/plugin machinery
- new provider families
- speculative workflow abstractions

They should not enter this hardening stack without a separate architecture
decision and workload that justifies them.
