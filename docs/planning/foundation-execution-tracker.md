# Foundation Execution Tracker

This tracker coordinates active work on
[`self-hosting-foundation-plan.md`](./self-hosting-foundation-plan.md)
and
[`scientific-workflow-foundation-plan.md`](./scientific-workflow-foundation-plan.md).
It is not a replacement for the plan, architecture docs, or subsystem
docs. It records who owns an active lane, which write set is reserved,
and what must be reviewed before merge.

Use this file for coordination only:

- Add a row before assigning a writer lane.
- Keep one writer per `hotspot:<crate>` lane unless the integrator has
  explicitly split the write set.
- Keep explorer and verifier work out of the active-writer table unless
  they are editing files.
- Do not mark a task `done` until the integrator has reviewed the
  actual diff, not just the writer summary.

## Active Slices

| Task ID | Owner | Branch | Worktree | Lane | Write Set | Reviewer | Verifier | Merge Order |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |

## Lane Status Notes

- Current branch: `main`.
- Current active worktree: `/Users/sinabooeshaghi/projects/frollo/belltower`.
- No parallel writer worktrees are active.
- Scientific workflow foundation planning is now tracked from
  `4cbddbe Add scientific workflow foundation plan`. Phase 0 snapshot
  verification confirmed the main baseline claims at that commit:
  `ContextManifest` is coarse, compaction could be recorded during
  context preparation before provider construction, approval projections
  retain thin request facts, `TurnOrchestrator` owns the turn loop,
  `bt-session` owns canonical projections/search, and `bt-otel`
  consumes committed canonical events.
- `scientific-p0-runtime-preflight` landed on
  `codex/science-foundation-p0-runtime-preflight`. Its invariant is that
  executable provider/auth/model preflight succeeds before runtime records
  model-visible context mutations such as `context.compacted`.
- `scientific-p1-turn-context` and `scientific-p1-context-surface` have landed
  the first context-manifest projection and consumer surface. Their invariant
  is that every provider call has an inspectable, projected model-visible
  context manifest with branch/source refs and explicit attachment slots for
  future memory/signals.
- `scientific-p2-compaction` landed the follow-up compaction slice. Its
  invariant is that successful compactions have stable identity and durable
  model-visible summary provenance that downstream inspection/export surfaces
  can reference.
- `scientific-p3-approval-snapshots` has landed. Its invariant
  is that pending approvals persist request-time evidence and resolved
  approvals persist a separate durable decision tied to the request
  fingerprint. New approval resumes must prefer the stored snapshot over live
  registry reconstruction while still executing the original canonical
  `tool.call.requested` arguments.
- Recent completed slices on this branch include provider/raw
  correctness (1.1/1.2), pricing/reprojection and instruction
  provenance (1.3a-e/1.4), budget and telemetry completion
  (3.1/3.2/3.3), auth storage backends (6.1/6.2/6.3),
  consumer-surface tests and examples (5.1/5.2/5.3/5.4/5.5 plus
  export round-trip coverage), distribution scaffolding (7.1/7.2), and
  install docs (7.3). The first deterministic acceptance runner for
  4.4 is also wired through `make acceptance`; M9 dogfood remains the
  final operational gate. Live M1/M2 provider turns are scripted through
  `scripts/live_acceptance.sh` and passed locally on 2026-04-24. 5.6
  deterministic TUI acceptance is wired through `make acceptance`; the
  full-screen M4.1 stress gate passed locally on 2026-04-24 with
  `BELLTOWER_TUI_ACCEPTANCE_TURNS=500`. Evidence is recorded in
  `docs/development/foundation-acceptance-log-20260424.md`.
- 4.2b turn orchestration relocation has landed: `bt-runtime` now owns
  the turn loop through `TurnOrchestrator`; `bt-server` keeps
  provider/auth/tool construction adapters and HTTP route behavior.
- 4.3 TUI invariant coverage and operator-surface readability are
  already covered by `docs/development/tui-invariant-coverage.md`,
  committed operator fixtures, and `cargo test -p bt-tui`.
- The scientific workflow foundation closeout gate is `make acceptance`.
  It now includes deterministic portable `session.bt`
  export/validate/import/diff/continue, artifact, raw-integrity,
  event-boundary branch fork, and filesystem CAS push/pull checks in
  addition to the server/runtime/TUI/export milestone proxies. Live
  provider, tmux stress, and dogfood gates remain explicit operational
  checks rather than default unit fixtures.
- 4.2a provider decomposition has removed the
  `crates/bt-providers/src/openai.rs` file-size override by moving
  OpenAI-compatible request translation into
  `crates/bt-providers/src/openai/request.rs`; stream parsing remains
  in the parent module.
- 4.2a client decomposition has removed the
  `crates/bt-client/src/lib.rs` file-size override by moving the
  inline client tests into `crates/bt-client/src/tests.rs`; the public
  client API and transport implementation remain in `lib.rs`.
- 4.2a TUI command-action decomposition has removed the
  `crates/bt-tui/src/command_actions.rs` file-size override by moving
  session/control mutating command helpers into
  `crates/bt-tui/src/command_actions/session_control.rs`; command
  dispatch and read-only command rendering remain in the parent module.
- 4.2a TUI command-render decomposition has removed the
  `crates/bt-tui/src/command_render.rs` file-size override by moving
  telemetry/raw inspection rendering into
  `crates/bt-tui/src/command_render/telemetry.rs`; general command
  rendering remains in the parent module.
- 4.2a launcher decomposition has removed the
  `crates/belltower/src/main.rs` file-size override by moving local
  server/TUI helper launch code into `crates/belltower/src/launcher.rs`
  and inline CLI tests into `crates/belltower/src/tests.rs`; setup,
  login, status, and provider-selection command flow remain in
  `main.rs`.
- 4.2a session-store decomposition has removed the
  `crates/bt-session/src/store.rs` file-size override by moving row
  parsing/encoding into `store/codec.rs`, branch/paging helpers into
  `store/paging.rs`, projection refresh logic into
  `store/projections.rs`, public record/projection DTOs into
  `store/types.rs`, and inline tests into `store/tests.rs`; the
  canonical SQLite store remains the single storage owner.
- 4.2a server decomposition has removed the
  `crates/bt-server/src/main.rs` file-size override by moving export
  and OTLP push routes into `crates/bt-server/src/exports.rs`, named
  operator-control routes into `crates/bt-server/src/operator_routes.rs`,
  tool execution/adapters into `crates/bt-server/src/tool_execution.rs`,
  and inline HTTP contract tests into `crates/bt-server/src/tests.rs`;
  `main.rs` remains the transport/router composition entrypoint.
- 4.2a runtime decomposition has removed the
  `crates/bt-runtime/src/runtime.rs` file-size override by moving
  lifecycle, session, event-recording, control-plane, context,
  query/export, inspection, lineage/workflow, service, budget helper,
  support, and type concerns into `crates/bt-runtime/src/runtime/`
  and runtime unit tests into `crates/bt-runtime/src/tests/runtime.rs`;
  `runtime.rs` remains the public root type and module map.
- File-size lint is now active through `make file-size` and CI, with
  current oversized files listed in
  `docs/development/file-size-overrides.md`. The list is an explicit
  coordination record for intentional temporary subsystem roots, not a
  mandate to introduce cosmetic modules solely to satisfy a line-count
  heuristic. Future 4.2 work should shrink oversized files only when a
  real ownership seam emerges, and should not add new entries without a
  plan update.
- The 4.2b design-before-code artifact is
  `docs/development/turn-orchestrator-interface-sketch.md`. It records
  the proposed `bt-runtime::TurnOrchestrator` seam, caller rewiring,
  and verification gates. The initial relocation follows that shape.
- No active writer lane is currently reserved. Before assigning a new
  writer, add a row in the Active Slices table with its branch, worktree,
  lane, and write set.
- Direction 0 snapshot refresh remains a per-PR requirement. If a
  snapshot claim drifts, land a snapshot-refresh change before the task
  branch and cite that SHA in the task's `Snapshot-verified:` trailer.
