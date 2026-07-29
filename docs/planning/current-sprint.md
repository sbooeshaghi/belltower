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

## Release Decision (2026-07-29)

**NO-GO for tagging `0.1.0` yet.** The code-critical blockers are closed:
exact-turn cancellation now interrupts provider, compaction, tool, shell, MCP,
and approval-resumed work without contradicting the canonical log, and
`web_fetch` pins each validated public DNS answer set through the actual
connection and every redirect.

Integrated-worktree acceptance passed the full deterministic workspace suite,
isolated source installation, live local and cloud providers, and the real
tmux flow with cancellation, steering, approval, inspection, and event export.
The remaining release bar is immutable exact-SHA evidence: repeat those lanes
from the clean release commit, record live OTLP-consumer ingestion and the full
M9 Belltower-on-Belltower dogfood period, and complete the manually dispatched
non-publishing five-target release-candidate workflow.

The release audit selected GitHub release archives and source-checkout builds
as the deliberate `0.1.0` distribution paths; all workspace crates set
`publish = false`, and CI verifies this source/archive contract and the local
three-binary install shape. Windows helper lookup honors `.exe`, tag and Cargo
versions must match, all platform artifacts must build before one release is
published, archives include the MIT license and checksums, and the
non-publishing release-candidate workflow runs the deterministic acceptance,
distribution, local-install, and five-target build gates. The full evidence and
exit checklist are recorded in
[`../development/release-readiness-20260729.md`](../development/release-readiness-20260729.md).

## Current Order

1. canonical identity and terminal lifecycle
2. atomic runtime ownership and cancellation
3. exact context, compaction, telemetry, and sync reconstruction
4. protocol, readiness, TUI, MCP, and delegated-session truth
5. workspace acceptance and final review

The latest completed consumer-truth slice removes default-branch inference
from cancel, steer, operator-command, and operator-shell writes. The protocol
now requires branch identity, the TUI snapshots invocation branch across
background work, and active-turn controls atomically reject cross-branch
attribution before append.

The related-session routing slice now removes `send_agent_message` default-
branch inference. Every new message names its target branch, replies must
reverse the original immutable session-and-branch edge, and runtime plus the
atomic store transaction reject conflicting routes without appending events.

The settings/budget provenance slice now removes the remaining known ambient
branch selection from operator writes. Both request DTOs require `branch_id`;
their projections remain session-scoped while their canonical update events
advance the explicitly addressed invocation branch. Protocol tests cover non-
default branches, foreign-branch rejection without mutation, restart
reconstruction, JSONL export, and queued TUI invocation snapshots.

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
