# Pre-Improvement Subsystem Review

Date: 2026-04-08

This review captures Belltower's pre-improvement state before the release
hardening stack that runs from `codex/p1-safety` through
`codex/p7-docs-parity`.

Treat this as a dated reference audit.
It explains why the later hardening plan existed, but it is not the source of
truth for current post-hardening behavior.
Use [`./current-sprint.md`](./current-sprint.md) for the active release gate
and [`../development/source-of-truth-matrix.md`](../development/source-of-truth-matrix.md)
for conflict resolution.

This review consolidates:

- the earlier repo audit findings
- the earlier harness landscape work in
  [`./harness-landscape-review-20260408.md`](./harness-landscape-review-20260408.md)
- subsystem-by-subsystem source inspection against
  [`../development/developer-guidelines.md`](../development/developer-guidelines.md),
  [`../development/source-of-truth-matrix.md`](../development/source-of-truth-matrix.md),
  and the current subsystem docs

Covered subsystems:

- [`../subsystems/core-protocol-and-server.md`](../subsystems/core-protocol-and-server.md)
- [`../subsystems/session-runtime-and-agent.md`](../subsystems/session-runtime-and-agent.md)
- [`../subsystems/providers-auth-and-models.md`](../subsystems/providers-auth-and-models.md)
- [`../subsystems/tools-context-and-approvals.md`](../subsystems/tools-context-and-approvals.md)
- [`../subsystems/launcher-and-tui.md`](../subsystems/launcher-and-tui.md)
- [`../subsystems/tui-architecture.md`](../subsystems/tui-architecture.md)
- [`../subsystems/tui-cutover-spec.md`](../subsystems/tui-cutover-spec.md)
- [`../subsystems/mcp.md`](../subsystems/mcp.md)
- [`../subsystems/telemetry-and-exports.md`](../subsystems/telemetry-and-exports.md)

## Top Conclusions

### `P0` Canonical control-plane state is still not actually canonical

This was the dominant pre-improvement blocker.

- Approval reuse was process-local rather than store-backed. Session/global
  approval scopes lived in runtime memory and were recreated empty after
  restart. See [`../../crates/bt-runtime/src/approval.rs`](../../crates/bt-runtime/src/approval.rs)
  and [`../../crates/bt-runtime/src/runtime.rs`](../../crates/bt-runtime/src/runtime.rs).
- Queued operator input, steer, and cancel state lived in `ControlQueues`
  memory and were only partially reflected in canonical history. See
  [`../../crates/bt-runtime/src/controls.rs`](../../crates/bt-runtime/src/controls.rs)
  and [`../../crates/bt-server/src/main.rs`](../../crates/bt-server/src/main.rs).
- Pause/resume sequencing was materially server-owned. Approval and `ask`
  resumption synthesized resumed turns and continued execution from server
  handlers instead of moving through a fully runtime-owned, replayable
  lifecycle. See [`../../crates/bt-server/src/main.rs`](../../crates/bt-server/src/main.rs).
- Inspection mixed store-backed projections with runtime-only state, which made
  `inspect`, `/queue`, and workflow views useful but not fully replayable
  truth. See [`../../crates/bt-runtime/src/runtime.rs`](../../crates/bt-runtime/src/runtime.rs)
  and [`../../crates/bt-core/src/types.rs`](../../crates/bt-core/src/types.rs).

Harness comparison: the strongest harnesses keep control-plane state explicit,
durable, queryable, and shared across clients. Pre-improvement Belltower was
not there yet for approvals, queued ingress, or pause/resume.

### `P1` Session-setting mutation is not cleanly defined relative to pause and queue state

- `/use`, `/connection`, and `/model` could be applied while a session was
  paused or had queued follow-up work.
- Approval resume paths reloaded current settings, while queued follow-up
  dispatch could continue using an older session snapshot.
- That meant one in-flight session could continue under inconsistent
  provider/model settings depending on which continuation path was taken.

Relevant code:

- [`../../crates/bt-tui/src/input_editor.rs`](../../crates/bt-tui/src/input_editor.rs)
- [`../../crates/bt-tui/src/command_actions.rs`](../../crates/bt-tui/src/command_actions.rs)
- [`../../crates/bt-server/src/main.rs`](../../crates/bt-server/src/main.rs)
- [`../../crates/bt-runtime/src/runtime.rs`](../../crates/bt-runtime/src/runtime.rs)

Before usability work, Belltower needed a clear policy for whether session
settings were frozen, versioned, or applied immediately across paused and
queued continuations.

### `P1` The TUI is still a co-owner of control state

The TUI was cleaner as a rendering system than as a control-plane consumer.

- It kept a local pre-queue before the canonical queue.
- It reconstructed pending approvals and asks from transcript heuristics instead
  of consuming one canonical runtime control-plane shape.
- It performed optimistic local cleanup during cancel and approval flows before
  authoritative events arrived.
- It presented approval scopes like durable operator commitments even though
  reusable scopes were not durable.

Relevant code:

- [`../../crates/bt-tui/src/input_editor.rs`](../../crates/bt-tui/src/input_editor.rs)
- [`../../crates/bt-tui/src/background_tasks.rs`](../../crates/bt-tui/src/background_tasks.rs)
- [`../../crates/bt-tui/src/command_actions.rs`](../../crates/bt-tui/src/command_actions.rs)
- [`../../crates/bt-tui/src/session_state.rs`](../../crates/bt-tui/src/session_state.rs)
- [`../../crates/bt-tui/src/pending_views.rs`](../../crates/bt-tui/src/pending_views.rs)

Harness comparison: mature harnesses keep the operator surface as a consumer of
control-plane truth, not a shadow owner of queue and approval semantics.

### `P1` Telemetry and export fidelity are weaker than the docs imply

- Approval-resume ordering could emit `tool.execution.finished` before the
  resumed `turn.started`, which could break the declared turn-root trace model
  in OTLP/export output. See [`../../crates/bt-server/src/main.rs`](../../crates/bt-server/src/main.rs)
  and [`../../crates/bt-otel/src/lib.rs`](../../crates/bt-otel/src/lib.rs).
- Export artifacts were built from `events + messages` only and did not carry
  standalone raw chunk payload fidelity or related-session/workflow lineage.
  See [`../../crates/bt-session/src/export.rs`](../../crates/bt-session/src/export.rs)
  and [`../../crates/bt-otel/src/lib.rs`](../../crates/bt-otel/src/lib.rs).
- Because queue and reusable approval state were not canonical, replay/export
  could not fully explain or reproduce important control-plane decisions.

This mattered because telemetry, replay, and export were one of Belltower's
strongest intended differentiators.

### `P1` Provider/auth/readiness truth still has correctness and ownership gaps

- Literal secrets that looked like env-var names could be reinterpreted as env
  references. See [`../../crates/bt-auth/src/lib.rs`](../../crates/bt-auth/src/lib.rs)
  and [`../../crates/belltower/src/main.rs`](../../crates/belltower/src/main.rs).
- Cloud readiness overstated usability by treating provider `/models` success
  as enough readiness without proving the configured default model was actually
  usable. See [`../../crates/bt-readiness/src/lib.rs`](../../crates/bt-readiness/src/lib.rs)
  and [`../../crates/bt-providers/src/openai.rs`](../../crates/bt-providers/src/openai.rs).
- Launcher messaging collapsed distinct readiness failures into "missing
  credentials."
- Readiness and local-model validation ownership was split between
  `bt-readiness` and launcher-local logic.

Harness comparison: onboarding and truthful readiness surfaces are a
first-class subsystem in strong harnesses. They are not just convenience output.

### `P1` MCP remains too flattened and observer-sensitive

- MCP session behavior leaned heavily on the same non-canonical approval reuse
  seam as the rest of the runtime.
- MCP status was partly derived from process-local discovery/cache state, so
  observing tools could change the visible lifecycle state.
- Every discovered MCP tool was flattened to the same high-risk,
  non-read-only, non-concurrency-safe contract, which was weaker than the
  tool-policy architecture wanted.

Relevant code:

- [`../../crates/bt-mcp/src/lib.rs`](../../crates/bt-mcp/src/lib.rs)
- [`../../crates/bt-server/src/inspection_tools.rs`](../../crates/bt-server/src/inspection_tools.rs)
- [`../architecture/tool-policy.md`](../architecture/tool-policy.md)

### `P1` Several subsystem docs were ahead of the code

The docs were directionally strong, but several described the intended
architecture more than the implemented one.

- Approval, queue, and control state was described as persisted and
  runtime-owned, while implementation remained hybrid and partly server-owned.
- `tui-cutover-spec` overstated how "cut over" queue and approval semantics
  were.
- `tui-architecture` was stale on some UI details such as idle footer behavior.
- `mcp.md` read as if streamable HTTP/SSE support was part of the active
  implementation shape.
- `providers-auth-and-models.md` did not define truthful behavior when settings
  changed during paused or queued work.

This drift mattered because the next phase was explicitly about subsystem
usability. The docs should not cause improvements to be built on stronger
assumptions than the code actually satisfied.

## Carried Forward From The Earlier Audit

These findings were carried into the hardening plan:

- Filesystem path containment bug for new paths in
  [`../../crates/bt-tools/src/filesystem.rs`](../../crates/bt-tools/src/filesystem.rs)
- Inconsistent not-found API semantics and bogus-session `200 OK` reads in
  [`../../crates/bt-server/src/main.rs`](../../crates/bt-server/src/main.rs)
  and [`../../crates/bt-core/src/error.rs`](../../crates/bt-core/src/error.rs)
- Blank endpoint token files blocking auth fallback in
  [`../../crates/bt-client/src/lib.rs`](../../crates/bt-client/src/lib.rs)
- Shell auto-approval substring smuggling in
  [`../../crates/bt-runtime/src/approval.rs`](../../crates/bt-runtime/src/approval.rs)
- Canonical write ordering issues around session creation, branch creation, and
  session settings updates in
  [`../../crates/bt-runtime/src/runtime.rs`](../../crates/bt-runtime/src/runtime.rs)

## Subsystem Notes

### Core Protocol / Server

- The server was still a control-plane owner, not just a transport and
  control-plane surface.
- The worst seam was the hybrid `SessionQueueInspection` view, which mixed
  canonical and live process-local state.

### Session Runtime / Agent

- Session/control truth was split across store-backed history, runtime memory,
  and server choreography.
- New and empty session identity was weaker than a fully event-backed session
  lifecycle.

### Tools / Context / Approvals

- Approval reuse, `ask`, queue, cancel, and resume should be treated as one seam
  problem.
- Until these were canonical, tool approval integrity remained weaker than the
  docs suggested.

### Providers / Auth / Models

- Fix auth literal/env ambiguity before deeper usability work.
- Centralize readiness and capability truth before changing provider UX.

### Launcher / TUI

- Remove local pre-queue and optimistic control cleanup once canonical control
  DTOs existed.
- Pause, cancel, and approval behavior needed truthful operator semantics
  before polish.

### TUI Architecture / Cutover

- Do not deepen transcript-heuristic pending-state reconstruction.
- Make the next TUI phase consume canonical control-plane inspection, or
  strengthen that inspection first.

### MCP

- Treat MCP as a stress-test for approval durability and tool metadata truth.
- Improve extension metadata shape before building heavier MCP workflows on top.

### Telemetry / Exports

- Fix resume ordering and export fidelity before positioning telemetry and
  replay as a dependable differentiator.

## Pre-Improvement Work Order

1. Canonicalize approval reuse, queued ingress, cancel/steer, and pause/resume
   state.
2. Define session-setting mutation semantics for paused and queued work.
3. Make TUI control state a consumer of canonical runtime state rather than a
   shadow owner.
4. Repair provider/auth/readiness truthfulness and ownership boundaries.
5. Align telemetry/export ordering and fidelity with the intended event model.
6. Update subsystem docs so they distinguish implemented truth from target
   architecture.
7. Then begin subsystem-specific usability work on top of those stabilized
   seams.

## Planning-Critical Test Gaps

The review repeatedly found the same missing verification classes:

- restart-persistence tests for approval reuse, queued messages, steer/cancel,
  and pending input state
- end-to-end approval and `ask` resume ordering tests
- TUI parity tests for queue, cancel, approval, reconnect, and pending-input
  recovery
- session-setting mutation tests during paused and queued work
- provider readiness tests that prove actual configured-model usability
- OTLP/export ordering tests for resumed turns
- export contract tests for raw chunk fidelity and standalone replay artifacts
- MCP end-to-end approval/resume tests with actual external tools

## Bottom Line

Belltower's architecture still pointed in the right direction.

The harness landscape review strengthened that conclusion: the right move was
not to collapse seams for convenience, but to finish hardening the explicit
seams Belltower already claimed to have.

The next phase therefore needed to start with control-plane canonicalization
and doc truthfulness, not UI polish layered on top of hybrid runtime state.
