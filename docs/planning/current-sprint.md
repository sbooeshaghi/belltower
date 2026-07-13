# Current Sprint: Foundation Integrity

This sprint hardens Belltower's reusable chat-agent envelope under concurrency,
interruption, restart, branching, and portable replay.

The active execution plan is:

- [`foundation-integrity-plan-20260713.md`](./foundation-integrity-plan-20260713.md)

Read it with:

- [`../architecture/overview.md`](../architecture/overview.md)
- [`../architecture/event-taxonomy-and-write-path.md`](../architecture/event-taxonomy-and-write-path.md)
- [`../development/developer-guidelines.md`](../development/developer-guidelines.md)
- [`../development/source-of-truth-matrix.md`](../development/source-of-truth-matrix.md)
- [`target-implementation-goals.md`](./target-implementation-goals.md)

## Sprint Outcome

Belltower should end this sprint with:

- session-scoped canonical projection identity
- atomic ownership of active turns and resumed control work
- terminal evidence for every requested tool
- tool results committed before later provider calls can consume them
- durable, exactly reconstructable compaction and context manifests
- telemetry derived from canonical request boundaries
- append-only remote trace advancement
- bounded readiness and truthful consumer/operator surfaces

## Current Order

1. canonical identity and terminal lifecycle
2. atomic runtime ownership and cancellation
3. exact context, compaction, telemetry, and sync reconstruction
4. protocol, readiness, TUI, MCP, and delegated-session truth
5. workspace acceptance and final review

The phases are dependency ordered. Later usability or feature work must not
weaken canonical event, control, and reconstruction invariants.

## Non-Goals

This sprint does not add autonomous scheduling, new memory policies, generic
hooks, compatibility layers, or new provider families. Complexity must solve a
measured failure before it enters the foundation.

## Working Rule

Docs, code, tests, and current behavior are all evidence during active
development. Architecture claims remain normative unless explicitly revised;
empirical claims must be verified against the current commit before editing.
