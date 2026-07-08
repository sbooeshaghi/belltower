# Belltower Platform Roadmap

This document translates the architecture and target-goals docs into an ordered platform roadmap.

Its purpose is not to replace [`./target-implementation-goals.md`](./target-implementation-goals.md).
That document defines the bar.
This document defines the order in which Belltower should reach that bar without eroding its architectural advantages.

Read it with:

- [`../architecture/overview.md`](../architecture/overview.md)
- [`../architecture/subagents-and-session-graphs.md`](../architecture/subagents-and-session-graphs.md)
- [`../architecture/datasets-and-training-artifacts.md`](../architecture/datasets-and-training-artifacts.md)
- [`./target-implementation-goals.md`](./target-implementation-goals.md)
- [`./current-sprint.md`](./current-sprint.md)

## Standard-Harness Outcome

To become the de facto standard for AI agent harnesses, Belltower needs more than a good chat loop.
It needs to be the strongest system in its class on these axes:

1. canonical runtime ownership
2. inspectable and exportable telemetry
3. workflow-graph clarity across related sessions
4. dataset and replay readiness
5. operator trust and truthful failure handling

The roadmap below is ordered around those leverage points.

## Phase 1. Telemetry Fidelity And Canonical Inspection

This phase is largely complete.

Delivered direction:

- canonical SQLite session and event ownership
- post-commit SSE and tracing fanout
- raw provider durability
- turn, trace, lineage, queue, and export inspection surfaces
- OTLP/OpenInference export from the canonical store

This phase made Belltower worth building.
It should not be reopened casually except for bug fixes and live-consumer validation.

## Phase 2. Workflow Graphs And Failure Semantics

This phase is largely in place.

Why it matters:

- a standard harness must stay understandable when multiple related sessions exist
- failure handling is part of product trust, not an implementation detail
- future delegation, replay, and dataset work all depend on stable workflow and error semantics

The intended outcomes are:

- explicit workflow inspection over related sessions
- delegation-ready canonical event taxonomy
- typed error taxonomy across runtime, protocol, TUI, and export
- mechanical enforcement of `bt-agent` purity
- live OTLP validation against a real consumer

This phase should stop short of autonomous spawning.
It should make that later work easier rather than prematurely implementing it.

The earlier coherence audit and release-hardening stack are now largely in
place, so the near-term priority shifts again: first close the release gate
cleanly, then strengthen the public consumer seams that sit on top of the
hardened control plane.
That ordering is intentional because it preserves the new runtime boundaries
instead of reopening them during polish.

## Phase 3. Dataset And Replay Foundation

Once workflow and failure semantics are stable, Belltower should move into dataset and replay construction.

Why this phase matters:

- this is where the telemetry moat becomes a self-improvement and research moat
- it is the bridge from "inspectable harness" to "user-owned agent trajectory platform"

The intended outcomes are:

- a dedicated dataset layer such as `bt-dataset`
- corpus selectors over sessions, branches, turns, tools, models, and lineage
- redaction and export-governance policies
- Belltower-native trajectory artifacts
- replay and evaluation artifacts
- protocol and TUI surfaces for dataset preview and export

This phase should remain derived from the canonical store.
It must not introduce a second runtime.

## Phase 4. Delegated Sessions And Workflow Execution

Only after workflow inspection and dataset/replay foundations are stable should Belltower add real delegated execution.

Why:

- child-session execution without strong workflow inspection becomes opaque quickly
- delegated work without dataset/replay support wastes one of Belltower's core advantages

The intended outcomes are:

- manual child-session spawn first
- structured handoff and result-import contracts
- workflow-level operator surfaces
- optional isolated worktree support for coding tasks
- later autonomous spawn policy on top of that

The architectural rule remains:

- a branch is not a subagent
- a subagent is a child session with its own runtime state

## Phase 5. Standalone Completeness And Ecosystem Polish

This phase turns Belltower from a strong harness into a mature product.

The intended outcomes are:

- truthful and capable local-model lifecycle management
- polished setup, login, doctor, status, and model flows
- continued hardening of endpoint-scoped local server auth and launcher/client reuse behavior
- stronger MCP lifecycle visibility and recovery behavior
- better web research ergonomics on top of canonical `web_search` and `web_fetch`, likely including additional backends and richer retrieval/extraction paths when workload evidence justifies them
- protocol compatibility discipline and golden tests
- TUI/CLI polish that keeps the operator fast without bypassing the protocol

This phase matters, but it should not come before the workflow and dataset foundations that differentiate Belltower from lighter harnesses.

In practice, some operator-facing work from this phase may be pulled earlier when it improves iteration speed without weakening the canonical runtime model.

## Things We Should Not Compromise

No roadmap pressure should erode these:

1. the canonical store is the source of truth
2. raw provider behavior remains durable and queryable
3. `bt-agent` stays pure and reusable
4. workflow state is explicit rather than inferred from transcripts
5. datasets are derived artifacts, not alternate runtime state
6. OpenInference and OTLP support remain projections over canonical Belltower data

## Recommended Near-Term Order

If development must stay pragmatic, the next few slices should be:

1. finish release closure on the hardening stack
2. protocol compatibility discipline, schema generation, and typed consumer artifacts
3. explicit host-bootstrap separation and `bt-server` library extraction
4. live OTLP validation against Phoenix or another real consumer
5. dataset/replay foundation
6. structured return artifacts and worktree policy for delegated sessions

That sequence keeps Belltower moving toward the strongest possible moat:

- one harness
- one canonical runtime
- one inspectable workflow graph
- one user-owned trajectory substrate for replay, export, and future training
