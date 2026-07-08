# Pre-Merge Review: 2026-04-10

This is a dated pre-merge review record for the release-hardening stack on
`codex/p7-docs-parity`.
Treat it as reference material, not as a source of current runtime truth.

## Scope

- compare `main...codex/p7-docs-parity`
- review the resulting code and docs against:
  - `AGENTS.md`
  - `docs/development/developer-guidelines.md`
  - `docs/development/source-of-truth-matrix.md`
  - `docs/architecture/overview.md`
  - `docs/architecture/event-taxonomy-and-write-path.md`
  - the relevant subsystem docs under `docs/subsystems/`

This pass was meant to answer two questions before merge:

1. what changed across the stacked hardening branches
2. whether those changes still match the architectural and development rules

## Branch Summary

The branch carries the seven planned hardening phases plus final audit-closure
fixes:

- `codex/p1-safety`
- `codex/p2-control-plane`
- `codex/p3-tui-launcher`
- `codex/p4-telemetry-export`
- `codex/p5-provider-auth-readiness`
- `codex/p6-mcp`
- `codex/p7-docs-parity`

The largest code movement landed in:

- `bt-runtime`
- `bt-server`
- `bt-session`
- `bt-tui`
- `bt-otel`
- release-facing docs and planning surfaces

## What Changed

### 1. Safety and correctness

- Tightened filesystem root containment and nested-path handling in
  `bt-tools`.
- Tightened shell auto-approval prefix matching so suffix-smuggled commands do
  not match approved patterns.
- Removed misleading blank-token and literal-secret/env ambiguity in the
  launcher/auth path.

### 2. Canonical control-plane seam

- Added durable `settings_revision_id` across session settings snapshots, turns,
  queued messages, pending approvals, pending input, and execution inspection.
- Added `TurnStartSource` and `resumed_from_call_id` so resumed and queued turns
  have explicit provenance.
- Made queue, cancel, steer, and reusable approval state durable in the SQLite
  store instead of process-local only.
- Made `SessionQueueInspection` the canonical pending-control DTO consumed by
  clients.
- Changed send/queue semantics so `/message` returns structured
  `dispatched`/`queued` outcomes instead of forcing clients to infer queue
  state.
- Tightened `/approve` so it only resolves a real pending resumable approval.
- Added startup recovery that closes interrupted resumed approval/input turns as
  failed instead of leaving the session stuck in `Working` forever after
  restart.

### 3. TUI and launcher adoption

- Moved TUI queue, approval, pending-input, and cancel surfaces onto canonical
  `SessionQueueInspection`.
- Kept transcript history display-only while control state comes from runtime
  inspection.
- Moved launcher readiness/model flows onto `bt-readiness` instead of separate
  local heuristics.
- Made `Ctrl-C` cancel follow the same canonical busy-state logic as the rest
  of the TUI.
- Updated TUI docs so the current quiet inline footer and
  `session_state.rs`/`pending_views.rs` ownership split match the implementation.

### 4. Telemetry and export fidelity

- Rebuilt resumed-turn OTLP/export provenance so resumed child tool work no
  longer appears before its parent turn.
- Added standalone canonical bundle export from the session store.
- Extended mirrored OpenInference/OTLP fields for turn provenance, approval
  scope outcomes, and resumed tool input/result relationships.

### 5. Provider/auth/readiness and consumer seam

- Added structured readiness states such as `MissingAuth`, `Degraded`,
  `Unreachable`, `ValidationFailed`, and `ConfiguredModelUnavailable`.
- Made launcher/status/model flows report those structured reasons instead of a
  coarse ready/not-ready interpretation.
- Kept remote client auth explicit and separated it from launcher-owned local
  token discovery.
- Added `docs/development/consumer-modes.md` to document the intended remote,
  launcher-local, and future host/bootstrap seams.

### 6. MCP lifecycle and inventory

- Added explicit MCP lifecycle truth including `configured`, `discovered`,
  `ready`, and degraded reporting.
- Added a canonical inventory surface and client/server support for `/mcp`.
- Kept MCP inventory as a consumer of runtime state rather than a separate
  heuristic layer.

### 7. Docs and planning parity

- Checked in the dated audit inputs that shaped the hardening plan:
  - `docs/planning/harness-landscape-review-20260408.md`
  - `docs/planning/pre-improvement-subsystem-review-20260408.md`
- Reclassified dated audit docs as reference-only in the source-of-truth docs.
- Rewrote `current-sprint`, `platform-roadmap`, and `subsystem-parity-matrix`
  to describe the post-hardening state rather than the old work queue.
- Fixed stale contract language in subsystem docs around not-found behavior,
  MCP surfaces, resumed-turn semantics, readiness, and TUI layout.

## Review Method

The final review was split by subsystem and checked in parallel against the
developer guidelines and the relevant architecture/subsystem docs:

- runtime/session/store/protocol control plane
- launcher/client/auth/readiness/consumer seams
- telemetry/export and server/API contracts
- TUI adoption and TUI docs parity
- MCP lifecycle plus planning/docs parity

Concrete findings from this pass were fixed before closing the review:

- wrong-tool-name approval resolution on `/approve`
- resumed-turn crash recovery on runtime startup
- `Ctrl-C` not respecting canonical runtime-busy state in the TUI
- remaining TUI ownership/docs-parity drift

## Outcome

After those fixes, the final review found no remaining unresolved code or docs
blockers in the stacked branch relative to the documented architecture and
developer guidelines.

Verification completed on the updated branch:

- `cargo fmt --all`
- `cargo check --workspace`
- `cargo test --workspace`

Residual non-blockers:

- pre-existing `bt-tui` dead-code warnings remain in workspace builds
- the live Arize/Phoenix OTLP test is still ignored without network and
  credentials

Remaining pre-merge process gate:

- the documented manual TUI and MCP smoke/acceptance checks in
  `docs/planning/current-sprint.md` are still outstanding

## Merge Read

From a code, docs, and architecture-parity standpoint, the hardening stack is
ready to merge after the remaining manual smoke/acceptance gate is satisfied.
