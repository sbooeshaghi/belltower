# Belltower Self-Hosting Foundation Plan

## Purpose

This plan defines the work to reach a stable, robust foundation on which
Belltower can be used autonomously and can be used to improve itself.

"Done" means an operator can:

- point Belltower at any supported provider and run a real multi-turn
  autonomous session with truthful readiness, truthful cost, enforced
  wall-clock budget, and durable raw-stream capture
- exercise the same HTTP/SSE surface from the built-in TUI or from an
  external UI through `bt-client`, without insider knowledge of Belltower
  internals
- audit any session six months later from canonical events and replay it
  or export it through derived formats with explicit fidelity
  boundaries: bundle export remains the full-fidelity raw-chunk
  artifact, while OTLP/OpenInference remains a canonical-event-derived
  view
- work on any single subsystem without needing to hold the rest of the
  workspace in working memory

Anything outside that goal is deferred except for the minimal markdown
instruction system required by FR-9, which is v1 and already partially
implemented.

This is a **foundation checkpoint plan**, not a replacement for the
full release target in `target-implementation-goals.md`. That document
remains normative for the complete release bar. This plan defines the
narrower foundation slice that should land cleanly before the remaining
target-bar work.

This plan is written against `main`. Every current-state claim in it has
been verified against code, not docs. For snapshot claims about what is
implemented today, code wins (per
[`../development/source-of-truth-matrix.md`](../development/source-of-truth-matrix.md)).
Architectural boundaries and long-lived invariants still come from the
normative architecture and subsystem docs. Principle 8 and
**Direction 0** below make snapshot refresh a standing pre-PR rule, not
a one-time kickoff ritual. Stale snapshots have caused overlapping work
before.

## Quick Start for Implementers

If you are an implementer (human or agent) newly assigned to a task,
follow this sequence:

1. Read §Current Code State Snapshot for the crates your task touches.
2. Find your task in §Work Directions. It has eight fields in a fixed
   order: Problem, Seam, Files touched, Depends on, Lane, Fix,
   Invariant, Exit test. Parse them in that order.
3. Check §Sequencing for the current week to confirm no upstream
   `Depends on` task is still in flight.
4. Check the execution tracker for active branches on your `Lane`. If
   another writer holds that `hotspot:<crate>` lane, wait or
   coordinate with the integrator.
5. Implement the task. Run `cargo fmt --all && cargo check --workspace
   --all-targets && cargo clippy --workspace --all-targets -- -D
   warnings && cargo test --workspace` plus the task's specific Exit
   test.
6. Add the `Snapshot-verified: <file:line or claim> still matches
   <short claim> as of <commit sha>` trailer to your commit. Cite the
   SHA of the latest snapshot-refresh (or `main` HEAD if none was
   needed).
7. Open the PR. Docs, targeted tests, snapshot trailer, and exit
   criterion must all be satisfied before merge.

Before your first task: read §Purpose, §Maintainability Principles,
and §Multi-Agent Execution Model. Expect ~30 minutes of onboarding;
subsequent tasks are much faster because the eight-field template is
identical across all tasks in the plan.

## Related Documents

Read these before starting any work in this plan:

- [`../../AGENTS.md`](../../AGENTS.md) — hard architectural rules
- [`../development/developer-guidelines.md`](../development/developer-guidelines.md)
- [`../development/source-of-truth-matrix.md`](../development/source-of-truth-matrix.md)
- [`./target-implementation-goals.md`](./target-implementation-goals.md)
  — FR-1..FR-19 and IR-1..IR-15 release-bar requirements
- [`./platform-roadmap.md`](./platform-roadmap.md)
- [`../architecture/overview.md`](../architecture/overview.md)
- [`../development/consumer-modes.md`](../development/consumer-modes.md)
  — the three-mode consumer seam definition
- the relevant subsystem docs under `docs/subsystems/` for the crates
  touched by the direction being executed

## Foundation Coverage Map

This plan captures the fundamental Belltower features from the root
project docs and architecture docs as foundation commitments:

- **Standalone Rust harness / workspace discipline:** covered by CI,
  file-size boundaries, crate ownership rules, `#![forbid(unsafe_code)]`
  expectations, and repo-independence discipline.
- **Canonical HTTP/SSE protocol boundary:** covered by OpenAPI
  generation, `bt-client` surface freeze, reference client, typed error
  matrix, and cross-process `Last-Event-ID` replay.
- **Session-as-telemetry / canonical SQLite store:** covered by raw
  chunk completeness, cost projection, budget checkpoints, sacred write
  path tests, canonical inspection truth, and export fidelity.
- **Provider/auth/model truth:** covered by the provider correctness
  tail, curated model discovery assumptions, coherent operator story,
  auth storage modes (file / keychain / ephemeral / auto) competitive
  with Codex's auth-storage story, and the optional Vertex expansion
  track.
- **Distribution:** covered by shareable GitHub release archives and a
  source-checkout Cargo install path, so dogfooders can hand the tool to a
  collaborator who does not already have a Rust toolchain without pretending
  the workspace is a public crates.io API.
- **Tools, approvals, MCP, and autonomy:** covered by the one runtime
  tool contract, named operations, M3/M7 acceptance, and the 5.4a
  inspection/tool-semantics contract.
- **TUI and launcher usability:** covered by TUI invariant coverage,
  long-session smoke, interactive operator-scenario smoke, coherent
  `setup`/`login`/`model`/`doctor`/`status` behavior, and the shared
  protocol/client path. **The TUI is the primary human interface
  during the foundation phase** — there is no external UI yet, no IDE
  integration, no web client — so TUI quality is load-bearing for
  dogfood validation. If the TUI has friction, dogfood week produces
  friction data against *both* Belltower (the system under test) and
  the TUI (the tool used to test it). The plan treats this as a
  compounding risk, not a cosmetic one.
- **Instructions and skills:** covered by FR-9 durable instruction
  provenance and M7.1 acceptance.
- **Session graphs, datasets, memory, signals, hooks, and
  self-improvement:** not implemented in this checkpoint, but protected
  by the foundation's canonical event log, lineage-aware inspection,
  raw evidence, named operations, and export surfaces. Those systems
  must remain derived from or routed through the foundation rather than
  creating alternate runtimes.

## Current Code State Snapshot

This section is authoritative for the plan's assumptions. If any claim
here is out of date when the work starts, stop and correct the plan
before proceeding.

**Already in place (do not redo):**

- ChatGPT OAuth refresh-aware resolution: `crates/bt-auth/src/lib.rs:292`
  (`resolve_fresh`), consumed per-turn at
  `crates/bt-server/src/main.rs:1762` via
  `require_runtime_credential_fresh`. Readiness uses the same path at
  `crates/bt-readiness/src/lib.rs:55`.
- ChatGPT device-code login is complete at
  `crates/belltower/src/main.rs:431` (`run_chatgpt_device_login`) with
  full OAuth device-code flow.
- Cost math is centralized in
  `crates/bt-runtime/src/runtime/support.rs:175`
  (`completion_cost_breakdown`). It returns `None` when no pricing
  entry matches. The per-provider `Provider::pricing` trait method is
  not the active cost path.
- **Pricing and reprojection contract is implemented:** ADR-0001
  (`docs/development/adr-0001-cost-projection-shape.md`) locks the
  non-null total plus `unpriced_completion_count` shape, token-class
  pricing fields, explicit unpriced-model rows, and
  operator-initiated reprojection policy. `ModelPricing` and
  `PricingCatalog` carry those shapes in `crates/bt-core/src/types.rs`
  and `crates/bt-core/src/config.rs`; `pricing.toml` includes priced
  rows plus explicit `[[unpriced_models]]` entries; `bt-session`
  persists and reads `unpriced_completion_count`; and
  `crates/bt-session/src/reprojection.rs` provides the reusable
  event-log walk seam.
- `EventPayload::BudgetCheckpoint` exists at
  `crates/bt-core/src/events.rs:198` with the full field set (tokens,
  turns, wall-clock, cost), and is mirrored to tracing at
  `crates/bt-otel/src/lib.rs:1813`.
- **Budget shape and runtime enforcement are implemented:** `BudgetConfig`
  is defined in `crates/bt-core/src/types.rs`, `BudgetConfigured` and
  `BudgetCheckpoint` are canonical events in
  `crates/bt-core/src/events.rs`, `session_budget_projection` exists
  in `crates/bt-session/src/migration.rs`, runtime configuration and
  checkpointing live in `crates/bt-runtime/src/runtime/sessions.rs`
  and `crates/bt-runtime/src/runtime/budget.rs`, and the server path
  calls runtime checkpointing after turn persistence.
  Restart-safe wall-clock budget tests live in
  `crates/bt-server/src/main.rs` and
  `crates/bt-runtime/src/tests/runtime.rs`.
- **Extended OpenInference token attributes are implemented:** the
  cache-read, cache-write, and reasoning token constants live in
  `crates/bt-core/src/events.rs`, OTLP emission lives in
  `crates/bt-otel/src/otlp.rs`, and export tests assert those
  attributes in `crates/bt-otel/src/tests.rs`.
- A live Arize/Phoenix OTLP round-trip test already exists at
  `crates/bt-server/src/main.rs:4188` (ignored by default; gated on
  env vars).
- The minimal `gcloud-adc` credential read is implemented at
  `crates/bt-auth/src/lib.rs:834`. It produces
  `CredentialKind::JsonDocument` with `refresh_secret: None`.
- Vertex catalog scaffolding is present in
  `crates/bt-core/data/providers/catalog.toml` (the `vertex`
  connection entry), `crates/bt-core/data/providers/pricing.toml` (the
  `gemini-2.5-pro` pricing row), and
  `crates/bt-core/data/providers/context_windows.toml`
  (the Gemini context window row).
- `bt-tui` is already modularized (~29 source modules); the TUI
  follows the Codex reference via
  [`../development/codex-tui-cutover-note-20260407.md`](../development/codex-tui-cutover-note-20260407.md).
  Real invariant coverage lives in `crates/bt-tui/src/tests.rs` and
  `crates/bt-tui/src/custom_terminal.rs`.
- Protocol fixtures are grouped by concern at
  `crates/bt-protocol/tests/fixtures/` (session_control / inspection /
  operator_surfaces / export / stream).
- The consumer seam is documented at
  [`../development/consumer-modes.md`](../development/consumer-modes.md).
  `bt-client` implements all three constructors.
- **FR-9 instruction provenance is implemented:** the
  `MarkdownInstructionResolver` at
  `crates/bt-runtime/src/instructions.rs:7` loads global + project
  markdown instructions and exposes typed accessors for the core prompt
  and provider overlays. Turn construction consumes that seam at
  `crates/bt-server/src/main.rs:1735`, and the server records durable
  `TurnInstructionProvenance` through
  `EventPayload::TurnInstructionProvenanceRecorded` before provider
  work begins. `bt-session` reconstructs provenance through the shared
  reprojection/event-log walk seam.
- **Readiness subsystem already handles unsupported providers honestly:**
  `crates/bt-readiness/src/lib.rs:225` returns
  `ConnectionReadinessState::RuntimeUnsupported` with status
  `"planned; provider not implemented yet"` for any connection whose
  provider has no runtime. Tested for `google-vertex` at
  `crates/bt-readiness/src/lib.rs:1128`.
- **Curated connection-model discovery is implemented** at
  `crates/bt-readiness/src/lib.rs:911` (OpenAI) and `:998` (Anthropic):
  provider inventories are filtered against the curated list in
  `catalog.toml` and surface `ConnectionModelSource::Discovered`.
- **Server metadata and capability reporting is implemented and
  tested:** `GET /server/info` returns `server_version`,
  `protocol_version`, `supported_protocol_versions`, and a rich
  `capabilities` struct (approvals, pending_input, session_queue,
  workflow_inspection, lineage_inspection, raw_chunk_paging, exports,
  mcp_inventory, mcp_reload, spawn_session). Test at
  `crates/bt-server/src/main.rs:2599`.
- **Typed `ErrorEnvelope` coverage exists:** bearer-auth failure path
  at `crates/bt-server/src/main.rs:2539`, 404-with-typed-envelope at
  `crates/bt-server/src/main.rs:2700`, and protocol-version-mismatch
  envelope nearby.
- **OpenAPI generation is implemented:** `bt-protocol` owns reusable
  schema components in `crates/bt-protocol/src/openapi.rs`, `bt-server`
  owns route metadata and document emission in
  `crates/bt-server/src/openapi.rs`, `make openapi` regenerates
  `docs/api/openapi.yaml`, and `make openapi-check` diffs the
  committed contract.
- **A package-local reference client exists** at
  `crates/bt-client/examples/reference_client.rs`, with run
  instructions at `docs/examples/reference-client.md`.
- **The error-envelope matrix is pinned** at
  `crates/bt-server/src/main.rs:2887`.
- **Cross-process SSE replay is tested** at
  `crates/bt-server/src/main.rs:6793`
  (`event_stream_replays_across_process_restart_without_duplicates`).
- **Canonical inspection minimum coverage exists** at
  `crates/bt-server/src/main.rs:6991`
  (`inspection_contract_min_reconstructs_canonical_session_truth`).
- **Export round-trip coverage exists** at
  `crates/bt-server/src/tests.rs`
  (`session_export_returns_legacy_bundle_jsonl_html_sharegpt_and_otlp`), with
  `make acceptance-exports` as the focused gate.
- **Same-process SSE `Last-Event-ID` replay under broadcast lag** is
  tested at `crates/bt-server/src/main.rs:5856`
  (`event_stream_recovers_from_broadcast_lag_by_replaying_store_events`).
- Anthropic reasoning-token extraction is implemented at
  `crates/bt-providers/src/anthropic.rs:481` and covered by
  `response_translation_tracks_usage_snapshots`.
- Provider secondary chunk raw provenance is implemented for OpenAI,
  ChatGPT, and Anthropic through provider-local `attach_raw_to_all`
  helpers; the server persistence assertion at
  `crates/bt-server/src/main.rs:3217` verifies committed completion
  chunks receive `raw_chunk_index` values.
- **Auth storage modes are implemented:** `bt-auth` has a
  `CredentialStore` trait plus file, ephemeral-env, feature-gated
  keychain, and auto stores under `crates/bt-auth/src/store/`.
  `belltower login --storage ...`, `belltower doctor`, and the
  auth-storage docs report the selected storage path and resolution
  order.
- **Distribution scaffolding is implemented:** `docs/install.md` documents
  source-checkout and release-archive paths; every workspace crate is marked
  `publish = false`; `scripts/distribution.sh` verifies that contract and the
  local install shape; and `.github/workflows/release.yml` builds release
  artifacts for tagged releases.
- **Acceptance runner is implemented:** `make acceptance` delegates to
  `scripts/acceptance.sh`, which runs deterministic milestone proxies
  for M0, M2.1, M3, M4, M5, M6, M7, M7.1, and M8 and explicitly
  skips live M1/M2 provider turns until a live-acceptance lane supplies
  local/cloud credentials. Scope is documented in
  `docs/development/acceptance-milestones.md`.

**Confirmed real gaps:**

- `crates/bt-providers/src/lib.rs:17` does not accept `google-vertex`.
  Readiness correctly returns `RuntimeUnsupported`. What is missing is
  the Vertex config/catalog contract first, then runtime admission, the
  Vertex provider, ADC refresh, and launcher UX for `JsonDocument`
  credentials.
- CI, `rust-toolchain.toml`, `clippy.toml`, and `make` targets exist.
  `make acceptance` now runs deterministic milestone proxies; live
  local/cloud provider gates and TUI/dogfood gates remain future
  acceptance work.
- **File-size lint is implemented:** `make file-size` and CI run
  `scripts/file_size_lint.sh`, which fails any non-test Rust source
  over 1,500 lines unless it has an explicit entry in
  `docs/development/file-size-overrides.md`.
- No source file currently exceeds the 1,500-line working-context cap
  without an explicit temporary override entry. Current overrides are
  tracked in `docs/development/file-size-overrides.md` as coordination
  records for intentional subsystem roots, not as permission to grow
  files indefinitely.
- Already split below the cap: `bt-otel`,
  `crates/bt-providers/src/openai.rs` (request translation moved into
  `openai/request.rs`), and `crates/bt-client/src/lib.rs` (inline tests
  moved into `src/tests.rs`), and
  `crates/bt-tui/src/command_actions.rs` (session/control command
  actions moved into `command_actions/session_control.rs`), and
  `crates/bt-tui/src/command_render.rs` (telemetry/raw rendering moved
  into `command_render/telemetry.rs`), and
  `crates/belltower/src/main.rs` (local helper launch moved into
  `launcher.rs` and inline tests moved into `tests.rs`), and
  `crates/bt-session/src/store.rs` (codec, paging, projection refresh,
  record/projection types, and tests moved into focused
  `store/*.rs` modules), and `crates/bt-server/src/main.rs` (exports,
  operator routes, tool execution/adapters, and inline tests moved into
  focused sibling modules), and `crates/bt-runtime/src/runtime.rs`
  (lifecycle, sessions, event recording, control-plane, context,
  query/export, inspection, lineage/workflow, services, support
  helpers, and internal types moved into focused `runtime/*.rs`
  modules; tests moved into `src/tests/runtime.rs`). The override list
  is now empty; future oversized source files require a plan update
  rather than implicit permission to grow.
- **Turn orchestration relocation has landed.** The semantic 4.2b move
  has introduced `bt-runtime::TurnOrchestrator`; `bt-runtime` now owns
  turn-id sequencing, `TurnStartSource` decisions,
  settings-revision routing, and `PostTurnControlAction` loop
  continuation. `bt-server` keeps provider/auth/tool construction
  adapters and HTTP route behavior. Remaining 4.2 work is verification
  hardening and preserving the bounded-context lint.
- **4.2b design gate is documented and the initial relocation has
  landed:** the required interface sketch
  lives at
  [`../development/turn-orchestrator-interface-sketch.md`](../development/turn-orchestrator-interface-sketch.md).
  It records the proposed `bt-runtime::TurnOrchestrator` public seam,
  adapter boundary, caller rewiring, what moves out of `bt-server`,
  and the post-relocation verification gates. The runtime now owns the
  turn loop through that seam; `bt-server` keeps provider/auth/tool
  adapters and HTTP route behavior.
- M0–M9 milestone tests from `target-implementation-goals.md` are not
  fully scripted against live local/cloud providers.
- `M4.1` long-session TUI smoke, `M9` dogfood evidence, and live M1/M2
  provider turns remain explicit acceptance gaps.

## Maintainability Principles

Binding on every task in this plan. Repeated from
[`../development/developer-guidelines.md`](../development/developer-guidelines.md)
because every task below is judged against them.

1. **Bounded working context.** A developer (human or agent) working on
   one subsystem should not need the rest of the workspace in their
   head. No file should exceed ~1,500 lines without a documented reason.
   Cross-crate coupling is explicit through traits, not implicit through
   shared state.
2. **Tests enforce invariants, not discipline.** Every implemented
   invariant this plan touches gets a mechanical check. Operational
   gates such as snapshot refresh or dogfood logs still require dated
   evidence, but logs do not substitute for the underlying tests.
   Examples already present:
   `crates/bt-agent/tests/purity.rs`,
   `crates/bt-protocol/tests/stabilization_fixtures.rs`.
3. **Single source of truth per concern.** Storage is `bt-session`.
   Protocol is `bt-protocol`. Turn orchestration and composition live
   in `bt-runtime`. `bt-server` owns transport and request handling
   around the runtime seam. Built-in tool execution is `bt-tools`.
   MCP transport and lifecycle are `bt-mcp`. Do not let a change make
   one crate shadow another's job.
4. **Smallest honest surface.** Each new capability exposes the minimum
   public API required for its consumer. Internal helpers stay
   `pub(crate)` or `#[doc(hidden)]`.
5. **Data shapes before behavior.** Define envelope types, event
   variants, DTOs, and trait signatures first; only then implement.
6. **Fail closed.** Storage unavailability, protocol version mismatch,
   readiness probe failure: all stop progress visibly. Never silently
   degrade.
7. **Telemetry is not optional.** If a change materially affects agent
   behavior, the session record must reflect it. No hidden paths.
   Session inspection is the primary operator surface for telemetry
   truth: if a human cannot reconstruct what happened from canonical
   inspection surfaces, the telemetry contract is incomplete.
8. **Verify before editing.** Every task begins by confirming the
   snapshot above is still accurate for the crate being touched. When
   stale, update this plan first. Every PR or commit that touches a
   crate covered by this plan should include a grep-able note in this
   form:
   `Snapshot-verified: <file:line or claim> still matches <short claim> as of <commit sha/tree state>.`
9. **Docs move with conceptual changes.** If a task changes crate
   ownership, canonical shapes, prompt/context composition, or
   operator-visible behavior, update the relevant architecture and
   subsystem docs in the same PR. In an actively developing project,
   code may be right, docs may be right, or both may need to move; the
   goal is restored coherence, not preserving the old shape. For
   architecturally normative claims, prefer making one explicit design
   decision in the relevant architecture/subsystem doc first and land
   the code/doc correction together. For empirical current-state claims,
   update this plan's snapshot as the forcing function.
10. **One runtime tool contract.** Built-in tools and MCP tools may
   differ in source, transport, and discovery, but once admitted into
   the harness they must follow the same runtime principles: canonical
   event emission, approval/control flow, inspection model, export
   semantics, and truthful degraded/error reporting. Differences in
   inventory or transport must not create a shadow execution model.
11. **Named operations, no hidden side effects.** Agent tools, MCP
   tools, operator commands, and runtime control transitions are all
   named operations with typed inputs and outputs at their owning seam.
   If an operation affects session behavior, workspace state,
   provider/runtime state, control flow, or operator-visible
   inspection/export state, it must emit or update canonical session
   state. UI-only display changes can remain local, but they must not
   masquerade as session truth. **Enforcement path:** the 5.4a-min
   inspection-reconstruction test asserts that the representative
   session contains a durable operator command reconstructed from
   canonical surfaces without transcript scanning. Any new operator
   command, tool source, or control transition added after the
   foundation ships must extend that test's named-operation
   inventory; silent additions that skip the test are a Principle 11
   violation, not a missing follow-on.
12. **Repo and app independence.** Belltower is a standalone harness,
   not an implementation detail of the surrounding Frollo workspace.
   Belltower crates must not depend on app-specific schemas, services,
   or product code. Integrations consume Belltower through
   `bt-protocol`, `bt-client`, MCP, or explicit custom-tool boundaries.

## Multi-Agent Execution Model

This plan is meant to move Belltower from single-maintainer execution
to reviewable parallel work by humans and agents. The operating model is
part of the foundation, not incidental process.

The current multi-agent model is deliberately conservative:

- The main agent or human maintainer is the integrator and owns
  cross-phase decisions.
- Every active writer works in a separate branch and, when practical, a
  separate worktree.
- For hotspot lanes, the stricter default applies: **one active writer,
  one dedicated worktree, one declared write set**. Do not stack
  multiple hotspot tasks in the same checkout.
- Default agent branch naming is `codex/foundation-<task-id>-<slug>`
  (for example `codex/foundation-1.3a-pricing-shape`). Human branches
  may use a user prefix, but should keep the task id.
- The execution tracker is mandatory for parallel work. At minimum it
  records: task id, branch name, worktree path, owner, reviewer,
  verifier, declared write set, lane, `Depends on` status, and intended
  merge order.
- A writer owns one reviewable slice at a time and declares its write
  set before editing.
- The crates and artifacts named below are **priority hotspots** —
  the ones this plan touches often enough that parallel writers will
  collide without explicit coordination: runtime, server,
  session-store, event/type definitions, provider catalog/pricing,
  and protocol DTOs. Each is single-writer unless the integrator has
  already landed a module map that makes the write sets disjoint.
- Any task-level `Lane` tag of the form `hotspot:<crate>` (e.g.
  `hotspot:bt-auth`, `hotspot:bt-otel`, `hotspot:bt-readiness`) means
  single-writer for that crate for the life of the task, even if the
  crate is not on the priority-hotspot list. The priority list names
  crates that are hot across the plan; the `hotspot:<crate>` Lane
  names a crate that is hot for one specific task. Both require
  coordination; the integrator treats both the same way.
- `Lane: isolated` means the write set does not overlap any active
  hotspot. These tasks can be assigned to any writer without
  coordination cost.
- `Lane: serial:<stack>` means the task is part of a named sequential
  stack (e.g. `serial:1.3*` for the pricing stack). Tasks within a
  stack must land in the order named by their `Depends on` field.
- Explorer or reviewer agents may inspect broadly, but writer agents
  must not make overlapping edits to the same hotspot.
- Reviewer and verifier are explicit roles, not an afterthought:
  reviewers examine the design/diff for seam discipline; verifiers run
  the stated tests, docs, and cross-subsystem checks after a writer
  slice lands, not after an entire week of work accumulates.
- Merge order follows the tracker, the task `Depends on` field, and any
  `serial:<stack>` lane. If two green slices are both mergeable, merge
  the one that establishes shared shape/contracts before the consumer
  slice that depends on them.
- The integrator must read the actual diff before marking a slice done.
  A writer or subagent summary describes intent; the diff is the
  outcome.
- No branch merges until its docs, targeted tests, snapshot verification
  note, and acceptance criteria are complete.

The future product-level agent framework is still experimental. The
foundation should not hardcode today's manual process as the permanent
answer. The durable rule is that any future "agent spawns agent" scheme
must itself use Belltower's canonical seams:

- spawned work is a named operation with typed input/output
- budgets apply to the spawned unit of work
- session lineage records parent/child relationships
- telemetry and inspection show what each agent did and why
- tool calls, approvals, errors, and exports remain canonical
- the same scheme works for one self-hosting agent and can extend to
  multiple cooperating agents without a second execution model

Delegated/subagent execution remains out of scope for this checkpoint,
but the foundation work must not make it harder. If a plan task touches
session lineage, budgets, tools, telemetry, or inspection, assume that a
future spawned-agent implementation will consume that seam.

## Work Directions

Each task below uses a fixed template. Agents and humans should parse
tasks by these exact fields in this exact order:

- **Problem.** Grounded in a file:line or named gap.
- **Seam.** Which crate owns the change, based on current code. Names
  the subsystem boundary, not the files.
- **Files touched.** The specific paths an implementer will edit or
  create. Used for conflict detection between parallel writers.
- **Depends on.** Other tasks in this plan that must land first, or
  "none" if independent. An implementer must verify these are green
  before opening their PR.
- **Lane.** One of: `isolated` (safe to parallelize), `hotspot:<crate>`
  (single-writer required), or `serial:<stack>` (ordered within a
  named sequential stack such as `1.3*`).
- **Fix.** What the change does. Bulleted specification, not a recipe.
- **Invariant.** Which foundation invariant (FR/IR/Principle) the
  change preserves.
- **Exit test.** The specific assertion or artifact that proves the
  task is done. Every exit test must name what is asserted, not just
  "test passes."

Do not extend this template with more fields without a plan edit. The
template is predictable structure an agent parses mechanically.

---

### Direction 0: Snapshot Refresh as a Pre-PR Requirement

This is Principle 8 applied operationally. It is not a one-time kickoff
task or an independent delivery track. Before opening or landing work
for a touched crate, re-verify the "Current Code State Snapshot" above
against the current tree. This prevents stale assumptions from
surviving into later weeks. Specifically:

- Confirm every "Already in place" citation still resolves to the
  claimed behavior (file:line can drift as modules move).
- Confirm every "Confirmed real gap" is still a gap (merged branches
  may have closed some).
- Add the `Snapshot-verified: ...` note to the PR or commit message
  using the fixed format above so the discipline is auditable.

**On drift.** When verification reveals that a snapshot claim no
longer matches code, do NOT bundle the snapshot correction into the
task PR. Open a **separate snapshot-refresh PR** that contains only
the corrected snapshot claims, land it first, and cite its commit SHA
in the subsequent task PR's `Snapshot-verified:` trailer. This keeps
the snapshot as a single-writer document and prevents parallel tasks
from fighting over conflicting snapshot edits. If the drift is purely
a line-number shift (no semantic change), a single-line snapshot PR
is acceptable; if the drift changes a gap classification ("already in
place" became "gap" or vice versa), the snapshot-refresh PR must also
revise any affected task bodies.

**Exit criterion.** Every PR touching a crate covered by this plan
either (a) is a snapshot-refresh PR that only updates drifted claims,
or (b) includes the fixed `Snapshot-verified: ...` note citing the
SHA of the latest snapshot-refresh (or `main` HEAD if no refresh was
needed) and confirms the relevant snapshot claims were re-checked and
remained accurate.

**This requirement applies throughout execution.** Every other
direction's "Problem" statement assumes the snapshot is current for the
crate being changed.

---

### Direction 1: Provider Correctness Tail

Narrowed to the gaps that remain on `main`. ChatGPT refresh, ChatGPT
device login, per-request credential resolution, curated model
discovery, and RuntimeUnsupported readiness handling are already done.

#### 1.1 Anthropic reasoning-token extraction

**Problem.** `AnthropicStreamState::usage_snapshot` at
`crates/bt-providers/src/anthropic.rs:480` hardcodes `reasoning_tokens: None`,
even though Anthropic extended-thinking responses carry
`output_tokens_details.reasoning_tokens` in the final usage block.
`update_usage` at the same file already reads `input_tokens`,
`output_tokens`, `cache_read_input_tokens`, and
`cache_creation_input_tokens`; reasoning tokens are the only gap.

**Seam.** `bt-providers::anthropic` (parser only).

**Files touched.** `crates/bt-providers/src/anthropic.rs`, plus one new
fixture file (captured Anthropic final-usage JSON) under
`crates/bt-providers/tests/fixtures/` or inline as a string literal in
the test module.

**Depends on.** none.

**Lane.** `hotspot:bt-providers` (Anthropic provider file). Serialize
against 1.2 because both tasks touch `crates/bt-providers/src/anthropic.rs`.

**Fix.** Read `output_tokens_details.reasoning_tokens` in `update_usage`;
surface it in `usage_snapshot`.

**Invariant.** FR-5 (OpenInference-aligned telemetry) and IR-12 (cost
accounting semantics).

**Exit test.** Unit test in `bt-providers::anthropic` that feeds a
captured Anthropic final-usage JSON with reasoning tokens and asserts
the resulting `TokenUsage.reasoning_tokens` is `Some(n)`.

#### 1.2 Raw-chunk completeness on secondary emission paths

**Problem.** Several chunk constructors in provider code emit
`CompletionChunk { raw: None, .. }`: `openai.rs:652` (structured-output
finalize), `chatgpt.rs:749` (tool-call flush), `chatgpt.rs:764`
(semantic tool-call bundle), `chatgpt.rs:773` (text-chunk helper).
Those are not the only offenders. Anthropic secondary text/reasoning/
tool deltas also emit `raw: None`, and `persist_live_completion_chunk`
only persists raw bytes when `chunk.raw.is_some()`. The result is that
supported providers can still write canonical `CompletionChunk` events
whose provenance is not durably linked back to raw provider output.

**Seam.** `bt-core` (if an explicit linkage shape is needed),
`bt-providers::openai`, `bt-providers::chatgpt`,
`bt-providers::anthropic`, and the `bt-server` persistence contract.

**Files touched.** `crates/bt-core/src/types.rs` (if raw-linkage shape
grows), `crates/bt-providers/src/openai.rs`,
`crates/bt-providers/src/chatgpt.rs`,
`crates/bt-providers/src/anthropic.rs`, and
`crates/bt-server/src/main.rs` around `persist_live_completion_chunk`.

**Depends on.** 1.1 must land first because both tasks touch
`crates/bt-providers/src/anthropic.rs`. Do not assign 1.1 and 1.2 to
different writers in parallel.

**Lane.** `hotspot:bt-providers` — all three provider files touched
by one writer; do not split.

**Fix.**

- Audit **all** secondary emitters in OpenAI, ChatGPT, and Anthropic,
  not just the currently named helper functions.
- Choose one shared raw-linkage contract in `bt-core` before editing
  providers: either every emitted chunk carries the upstream raw payload
  directly in `raw`, or synthesized boundaries carry explicit canonical
  linkage metadata. No supported-provider path leaves provenance
  ambiguous.
- Update provider emitters and the persistence path together so every
  committed `CompletionChunk` has either persisted raw bytes or explicit
  linkage to the raw source.

**Invariant.** FR-6 (raw stream durability).

**Exit test.** Provider-level unit tests stream a synthetic
tool-and-text turn through OpenAI, ChatGPT, and Anthropic and assert
every emitted chunk has either `raw.is_some()` or the new explicit
linkage metadata. Add one integration assertion at the persistence seam
showing that committed chunks do not lose raw provenance.

#### 1.3 Pricing contract and reprojection track

**Current status.** Completed on the active foundation branch. The
subtasks below remain as traceability records for the completed stack.
Do not reopen or reimplement them unless a fresh Direction 0 snapshot
shows drift.

This direction is intentionally split into reviewable slices. Do not
land it as one giant "pricing overhaul" PR.

##### 1.3a ADR + pricing shape expansion

**Problem.** `TokenUsage` and `CostBreakdown` already model cache-read,
cache-write, and reasoning token classes, but `ModelPricing` and the
pricing catalog do not. The projection also cannot currently express the
difference between "free" and "unpriced" without an explicit contract.

**Seam.** `bt-core` (pricing shapes) plus the ADR under
`docs/development/`.

**Files touched.**
`docs/development/adr-0001-cost-projection-shape.md` (new, copied
from `docs/development/adr-template.md`),
`crates/bt-core/src/types.rs` (expanded `ModelPricing` shape),
`crates/bt-core/src/config.rs` if `pricing.toml` parsing grows, and
test files under `crates/bt-core/tests/` or inline test module.

**Depends on.** none.

**Lane.** `serial:1.3*` — head of the pricing stack. 1.3b/c/d/e wait
for this ADR to land.

**Fix.** Before writing code, record the pricing/projection contract in
`docs/development/adr-0001-cost-projection-shape.md`. The ADR must
cover both the **schema decision** and the **reprojection-trigger
policy** decision in the same document, because they get locked in
together once 1.3c ships:

- Schema: compare the three projection options (nullable total, flag
  column, dual counters). Keep the current recommendation explicit:
  Option B, retaining `total_cost_usd REAL NOT NULL DEFAULT 0` and
  adding `unpriced_completion_count`.
- Pricing shape: expand `ModelPricing` so it can represent token-class
  pricing where public rates exist and explicit unpriced reasons where
  they do not.
- Reprojection trigger policy: decide explicitly *when*
  reprojection/backfill runs against existing sessions. Options
  to compare:
  - **Operator-initiated only** (slash command or CLI subcommand;
    safe default; no automatic mutation of historical sessions).
  - **Auto on catalog-hash change** (catalog change implicitly repairs
    history; requires content-hash on `pricing.toml` and a startup or
    background reprojection pass).
  - **Manual + audit-only** (catalog change emits a notification; no
    repair without explicit operator action).
  Pick one. Document the consequence: IR-12 requires historical session
  cost surfaces to stay stable as the pricing catalog evolves; the
  trigger policy decides what "stay stable" means in practice.

**Invariant.** IR-12 and Maintainability Principle 5 (data shapes
before behavior).

**Exit test.** ADR lands documenting both schema and reprojection
trigger policy, `bt-core` pricing-shape tests compile, and the chosen
data shape and trigger policy are both explicit before any
store/runtime changes.

##### 1.3b Catalog coverage + coverage test

**Problem.** `crates/bt-core/data/providers/pricing.toml` has 3 entries while
`catalog.toml` references ~15 models. Every unpriced model silently
produces no `CostBreakdown`.

**Seam.** `crates/bt-core/data/providers/pricing.toml` plus a `bt-core` coverage test.

**Files touched.** `crates/bt-core/data/providers/pricing.toml` (expanded +
`[unpriced_models]` table), `crates/bt-core/tests/` (new coverage
test).

**Depends on.** 1.3a (pricing shape must be finalized first so the
`[unpriced_models]` schema exists).

**Lane.** `serial:1.3*` — after 1.3a.

**Fix.**

- Expand `pricing.toml` to cover every `default_model` and
  `model_fallbacks` entry in `catalog.toml` for which public pricing is
  known.
- Add a new `[unpriced_models]` table listing models explicitly known to
  be unpriced, each with a `reason` string.
- Add a `bt-core` test that walks `catalog.toml` and asserts every
  referenced model is either priced or listed in `unpriced_models`.

**Invariant.** IR-12 and FR-2 (truthful capability reporting).

**Exit test.** Catalog coverage test passes and no referenced model is
silently uncategorized.

##### 1.3c Projection schema + writer update

**Problem.** The cost projection schema at
`crates/bt-session/src/migration.rs:109` stores
`total_cost_usd REAL NOT NULL DEFAULT 0`, and the writer at
`crates/bt-session/src/store.rs:1573` currently maps `cost: None` to
`0.0` via `cost.as_ref().map_or(0.0, |cost| cost.total_usd)`. An
unpriced session and a genuinely-$0 session therefore look identical.

**Seam.** `crates/bt-session/src/migration.rs` and
`crates/bt-session/src/store.rs`.

**Files touched.** `crates/bt-session/src/migration.rs` (new migration
step), `crates/bt-session/src/store.rs` (projection writer update),
test files under `crates/bt-session/tests/` or inline.

**Depends on.** 1.3a (ADR-selected schema shape) and 1.3b (catalog
coverage so tests have both priced and unpriced cases).

**Lane.** `serial:1.3*` — after 1.3a/1.3b. Also `hotspot:bt-session`
— do not parallelize with 1.3d (same crate).

**Fix.**

- Apply the ADR-selected schema change in `bt-session` (currently Option
  B: add `unpriced_completion_count` with a migration).
- Update the projection writer so unpriced completions increment the
  counter and priced completions continue to accumulate dollars.

**Invariant.** IR-12 honest cost accounting.

**Exit test.** `bt-session` projection tests show an unpriced turn
increments `unpriced_completion_count` and leaves `total_cost_usd`
unchanged, while a priced turn does the opposite.

##### 1.3d Reprojection/backfill harness

**Problem.** `CompletionFinished.cost` is computed once at append time,
so adding pricing rows later does not repair already-recorded sessions
unless the plan includes reprojection/backfill explicitly.

**Seam.** `bt-session` owns the reusable event-log walk harness. Create
one new module for it (for example
`crates/bt-session/src/reprojection.rs`). Direction 1.4 consumes this
harness; no parallel event-log walker appears in `bt-runtime` or
elsewhere.

**Files touched.** `crates/bt-session/src/reprojection.rs` (new),
`crates/bt-session/src/lib.rs` (add module), possibly
`crates/bt-session/src/store.rs` for reprojection entry points, tests
under `crates/bt-session/tests/`.

**Depends on.** 1.3c (projection schema must exist before reprojection
can repair it).

**Lane.** `serial:1.3*` — after 1.3c. `hotspot:bt-session`.

**Fix.**

- Add an explicit reprojection/backfill path over stored
  `CompletionFinished` events so pricing-catalog updates can repair
  existing sessions.
- Make the harness reusable by other reconstruction tasks rather than
  embedding pricing-specific walk logic directly into one migration or
  runtime helper.

**Invariant.** Maintainability Principle 3 (single source of truth per
concern).

**Exit test.** A reprojection test proves an existing session with newly
added pricing data is repaired from stored events.

##### 1.3e Inspection surface + runtime cost breakdown

**Problem.** Operators cannot currently see priced totals and unpriced
counts clearly, and token-class pricing remains invisible even when the
usage model carries the necessary fields.

**Seam.** `bt-core` (shared inspection cost data shape),
`bt-runtime` (cost breakdown computation and emitted summary),
`bt-protocol` / inspection DTO fixtures, and the `bt-session` read path
those DTOs consume.

**Files touched.** `crates/bt-runtime/src/runtime.rs`
(`completion_cost_breakdown` around `:3276` plus inspection emission),
`crates/bt-core/src/types.rs` (shared inspection cost-summary shape),
`crates/bt-server/src/main.rs` (inspection route contract assertion),
`crates/bt-protocol/tests/fixtures/session_control.json` or
`inspection.json` (new round-trip fixture).

**Depends on.** 1.3a (pricing shape), 1.3c (projection schema), 1.3d
(reprojection for sessions written before token-class pricing).

**Lane.** `serial:1.3*` — tail of the stack. Crosses `hotspot:bt-runtime`
and `hotspot:bt-protocol`; serialize against 3.1 (which also touches
bt-protocol session_control fixtures).

**Fix.**

- Add cost-summary fields to the canonical inspection surface so
  operators can see priced totals and unpriced counts.
- Ensure token-class pricing contributes to `CostBreakdown` when
  configured.
- If a provider reports cache/reasoning token classes but public pricing
  is unavailable, surface that explicitly as unpriced rather than
  silently dropping the fields.

**Invariant.** IR-12 and FR-2.

**Exit test.** Runtime cost-breakdown and inspection-surface tests are
green.

#### 1.4 FR-9 instruction-system verification

**Current status.** Completed on the active foundation branch. The
canonical turn-scoped instruction provenance shape exists, is persisted
before provider work, and is reconstructable from the session event log
through the shared `bt-session` reprojection seam. The task body below
is retained as the original acceptance contract.

**Problem.** FR-9 in
[`./target-implementation-goals.md`](./target-implementation-goals.md)
requires a minimal markdown instruction system with core + global +
project scopes, deterministic composition, and reconstructable active
set per turn. The `MarkdownInstructionResolver` at
`crates/bt-runtime/src/instructions.rs:7` is live and is consumed
during turn construction at `crates/bt-server/src/main.rs:1735`.
Earlier snapshots identified the lack of canonical stored provenance as
the gap; the active branch has closed that gap.

**Scope boundary.** FR-9 scope in this task is limited to **canonical
instruction provenance for the existing v1 markdown instruction
system** — the core prompt, provider overlays, global markdown
directory, and project markdown directory that
`MarkdownInstructionResolver` already loads. Skill authoring
workflows, `/improve`, skill-attachment events, skill-proposal
artifacts, and any self-mutation of skills are **out of scope** here
and belong to the post-foundation `bt-improve` / skills track listed
under Out of Scope. An implementer over-reaching into those areas is
scope creep, not FR-9 completion.

**Seam.** `bt-core` / `bt-session` (canonical instruction provenance
shape), `bt-context` (effective prompt/input assembly surfaces such as
managed prompt assets, runtime context, and tool guidance),
`crates/bt-runtime/src/instructions.rs` (composition logic), and the
turn-start consumption path around
`crates/bt-server/src/main.rs:1735`. Prompt assets live under
`data/prompts/`. This is **not** a telemetry task.

**Files touched.** `crates/bt-core/src/events.rs` (new event variant
for instruction provenance, if that is the chosen shape),
`crates/bt-session/src/store.rs` (persistence of the new event or
projection), `crates/bt-context/` (assembly surface touch-up),
`crates/bt-runtime/src/instructions.rs` (composition logic),
`crates/bt-server/src/main.rs` around `:1735` (emit the provenance
record at turn start), and a new reconstruction test under
`crates/bt-runtime/tests/` or `crates/bt-session/tests/`.

**Depends on.** 1.3d (reuses the `bt-session` event-log walk/
reprojection harness; do not introduce a parallel walker here).

**Lane.** `hotspot:bt-core` and `hotspot:bt-session` — touches event
type definitions and storage. Serialize against 3.1 (which also
touches `bt-core` event variants) and 1.3c/1.3d (which touch
`bt-session`).

**Fix.**

- Define a canonical turn-scoped instruction provenance shape before
  editing resolver behavior. This can be an event payload or equivalent
  stored record, but it must be durable enough to reconstruct the active
  instruction set for a historical turn from session state.
- Audit the full FR-9 seam against the current v1 prompt/input
  composition: core prompt, provider overlays, global skills directory,
  project skills directory, generated runtime context, generated tool
  guidance, deterministic composition order, and observable active set.
- If an FR-9 requirement is not satisfied, patch it in
  `bt-context`, `crates/bt-runtime/src/instructions.rs`, and the new
  storage seam as appropriate. Do not relocate this work into budget
  telemetry or other unrelated subsystems.
- Reuse the `bt-session` event-log walk/reprojection harness established
  in 1.3d rather than introducing a second parallel reconstruction
  utility.
- Add a test that constructs a turn with a known combination of global
  + project skills plus generated prompt/context sections, persists the
  canonical instruction provenance, then reconstructs the active set
  from stored state and asserts equivalence.

**Invariant.** FR-9 (instructions and skills), Maintainability
Principle 3 (single source of truth per concern).

**Exit test.** Canonical instruction-provenance shape lands with
storage/protocol coverage where needed; the reconstruction test passes;
an audit note may accompany the change but is not the acceptance gate.

---

### Direction 2: Vertex Provider Expansion Track (Optional for Foundation v1)

Vertex is **not** in the foundation Definition of Done. It is a
clearly-scoped expansion track that can run alongside the rest of the
plan only with an explicit non-overlapping write lane. Pulling Vertex
into DoD alongside monolith splits and CI is too large a bite and blurs
the foundation.

Current state: catalog, pricing, and context-window scaffolding exist
(`crates/bt-core/data/providers/catalog.toml`, `pricing.toml`,
`context_windows.toml`)
plus ADC parsing at `crates/bt-auth/src/lib.rs:834`. Readiness already
reports `RuntimeUnsupported` honestly. What is still missing is the
Vertex-specific config contract, runtime admission + provider,
provider-facing ADC refresh policy, curated model selectors, and the
launcher/setup UX.

**Gating.** **2.1 must land before 2.3 or 2.4 have any end-to-end
consumer.** `2.2` can already be exercised through readiness because
`resolve_fresh()` is called there today, but its provider-facing
credential policy must be chosen in lockstep with `2.1`. Build `2.1`
first; then `2.2` / `2.3` / `2.4` can run behind it with explicit seam
contracts.

If Vertex is required for the operator experience you want at the end
of this arc, run this track. Each task is sized independently and can
be paused between tasks without leaving the tree in a broken state.
Because it shares catalog, pricing, provider, readiness, auth, and
launcher surfaces with other directions, assign it a named owner,
reserved lane, and explicit merge order before editing those shared
files. If those shared lanes are already occupied by Direction 1 or
Direction 6 work, defer Vertex rather than overlap the edits.

#### 2.1 Vertex config contract + runtime provider admission

**Problem.** `crates/bt-providers/src/lib.rs:17` does not admit
`google-vertex` in `connection_supported`, and no `vertex.rs` module
exists. More importantly, the current connection/config seam has
nowhere to carry Vertex-specific `project` / `location` data, even
though the plan expects provider validation and `doctor` output to use
them.

**Seam.** `bt-core` config/catalog contract + `bt-providers`.

**Files touched.** `crates/bt-core/src/config.rs`
(`ConnectionDescriptor` extensions), `crates/bt-core/data/providers/catalog.toml`
(Vertex connection entry extended with project/location fields),
`crates/bt-providers/src/vertex.rs` (new module),
`crates/bt-providers/src/lib.rs:17` (admit `"google-vertex"`), new
fixture under `crates/bt-providers/tests/fixtures/` or inline.

**Depends on.** none (but coordinate with 1.3a if pricing shape is
still settling, because the Vertex connection entry references
`gemini-2.5-pro` pricing).

**Lane.** `hotspot:bt-providers` and `hotspot:bt-core` — single writer
for both. Coordinate with 1.3* stack if either is active.

**Fix.** First define the canonical Vertex configuration contract in the
connection/catalog layer, then implement provider admission on top of
it:

- Extend `ConnectionDescriptor` / `catalog.toml` with explicit
  Vertex-specific configuration sufficient for provider validation and
  launcher status reporting.
- Add new module `vertex.rs` implementing the `Provider` trait against
  the Google GenAI REST surface.
- `vertex.rs::validate()` hits the Vertex models-list endpoint for the
  configured project/location.
- `vertex.rs::list_models()` returns discoverable Gemini models.
- `vertex.rs::stream_completion()` uses `streamGenerateContent` SSE.
- Map Gemini parts (`text`, `functionCall`, `functionResponse`,
  `thought`) to the canonical `CompletionDelta` enum.

Constructor accepts `RuntimeCredential::JsonDocument` (ADC) or
`BearerToken` (direct access token); rejects `ApiKey`.

**Invariant.** Provider compatibility discipline.

**Exit test.** Config parsing test plus fixture-driven round-trip test
with a recorded Gemini stream.

#### 2.2 ADC token refresh in `bt-auth`

**Problem.** `crates/bt-auth/src/lib.rs:834` reads
`application_default_credentials.json` but sets `refresh_secret: None`.
Access tokens expire; refresh never happens. The plan also needs one
explicit credential-shape decision: whether refreshed ADC stays
`JsonDocument` through the provider seam or resolves eagerly to a
bearer-like runtime credential with matching readiness semantics.

**Seam.** `bt-auth` owns ADC refresh mechanics; `bt-readiness` and the
eventual Vertex provider are consumer seams that must stay on the same
credential contract.

**Files touched.** `crates/bt-auth/src/lib.rs` around `:292`
(`resolve_fresh`) and `:834` (`resolve_gcloud_adc`), plus synthetic
ADC JSON fixtures under `crates/bt-auth/tests/` or inline.

**Depends on.** 2.1 (credential-shape decision is entangled with
provider admission — ADC may stay `JsonDocument` through the provider
seam or resolve eagerly to bearer; pick in lockstep with 2.1).

**Lane.** `hotspot:bt-auth`.

**Fix.** Extend `resolve_fresh` to recognize ADC credentials and make
the provider-facing credential policy explicit:

- User credentials: exchange the persisted refresh token at the Google
  OAuth token endpoint.
- Service-account keys: mint a JWT-bearer assertion and exchange for a
  short-lived access token.
- 5-minute expiry buffer, matching the Gemini CLI convention.
- Keep `bt-auth`, `bt-readiness`, and the eventual Vertex provider on
  the same chosen credential shape; do not let one layer assume
  `JsonDocument` while another assumes bearer-only.

**Invariant.** FR-3 (first-class auth), IR-5.

**Exit test.** `bt-auth` tests with synthetic ADC JSON files covering
both user credentials and service-account keys.

#### 2.3 Vertex readiness transitions past `RuntimeUnsupported`

**Problem.** `crates/bt-readiness/src/lib.rs:225` already returns
`RuntimeUnsupported` for Vertex because `bt-providers::connection_supported`
rejects `"google-vertex"`. Once 2.1 lands, `connection_supported` will
admit Vertex and readiness must then handle the real state transitions:
`MissingAuth`, `UnsupportedAuthMethod`, `Unreachable`, `Degraded`,
`ConfiguredModelUnavailable`, `Ready`. The code path in `bt-readiness`
already applies the curated-model filter generically; what is missing
is Vertex selector data in `catalog.toml`.

**Seam.** `bt-providers::lib.rs` (add Vertex to `connection_supported`),
`bt-readiness`, and `crates/bt-core/data/providers/catalog.toml`.

**Files touched.** `crates/bt-providers/src/lib.rs` (admit Vertex;
coordinates with 2.1), `crates/bt-core/data/providers/catalog.toml`
(`discoverable_model_selectors` for Gemini),
`crates/bt-readiness/src/lib.rs` (test expansion — the state machine
itself does not need a Vertex-specific branch), tests around `:1128`.

**Depends on.** 2.1 (Vertex must be admitted by `connection_supported`
before readiness can transition past `RuntimeUnsupported`). Also
benefits from 2.2 if ADC refresh is exercised, but not strictly
required.

**Lane.** `hotspot:bt-providers` and `hotspot:bt-readiness`.

**Fix.**

- Add Vertex to `connection_supported` in lockstep with 2.1.
- Add `discoverable_model_selectors` for Vertex / Gemini in
  `catalog.toml`; reuse the existing provider-agnostic curated-model
  filter path rather than introducing a Vertex-specific branch.
- Verify the existing readiness state machine applies correctly to
  Vertex without a new Vertex-specific branch.

**Invariant.** FR-2 truthful readiness.

**Exit test.** Extend `bt-readiness` tests with mocked Vertex responses
covering the full state matrix. The existing
`unsupported_provider_marks_runtime_unsupported_state` test at
`crates/bt-readiness/src/lib.rs:1128` should be updated to assert Vertex now
reaches `Ready` when auth + reachable + model-available.

#### 2.4 Launcher UX for `JsonDocument` auth

**Problem.** `crates/belltower/src/main.rs:391` branches on device-code vs
api-key only. A `belltower login vertex` attempt hits
`ensure_connection_supports_api_key_login` and fails. The interactive
login and setup flows also filter connection choices assuming only
API-key or device-code auth, so Vertex is hidden before `run_login()`
even runs.

**Seam.** `belltower` (launcher binary).

**Files touched.** `crates/belltower/src/main.rs` around `:391`
(`run_login` branch for `JsonDocument`) and the interactive
connection-choice/setup flows in the same file.

**Depends on.** 2.1 (config contract must carry project/location) and
2.2 (refreshed ADC credential shape must be finalized so `doctor` can
report it honestly).

**Lane.** `hotspot:belltower`.

**Fix.** Add a `JsonDocument` branch in `run_login`:

- Detect existing ADC; offer to use it.
- If `gcloud` is on PATH, offer to run
  `gcloud auth application-default login`.
- Accept a `--credentials-file <path>` flag pointing at a
  service-account JSON; copy to the auth store.
- Update the interactive connection-choice and setup flows so
  `JsonDocument`-backed providers are visible and handled truthfully.

`belltower doctor` reports which path is active and the resolved
project / location / default_model.

**Invariant.** FR-1 (coherent launcher), FR-3.

**Exit test.** Acceptance script: fresh install → `belltower login
vertex` → `belltower doctor` shows Vertex Ready → first turn succeeds
against a real Vertex endpoint.

---

### Direction 3: Budget Enforcement + Telemetry Completion

The budget shape and enforcement path have landed on the current
foundation branch: canonical budget config/events, the
`session_budget_projection`, runtime checkpointing, and server turn-path
hooks are present. Directions 3.1 and 3.2 remain below as traceability
records for the invariants and exit tests; do not reimplement them
unless a fresh snapshot shows drift. Remaining Direction 3 work is the
pricing/catalog accuracy from Direction 1.3 and any follow-on coverage
gaps found by 5.4a-ext.

#### 3.1 Canonical budget shape

**Problem.** There is no `BudgetConfig` struct in `bt-core`, no
persistent budget state on `SessionRecord` or a new projection, and no
protocol surface on `CreateSessionRequest` / `UpdateSessionRequest` to
configure per-session budgets.

**Seam.** `bt-core` (types), `bt-session` (projection + migration),
`bt-protocol` (DTOs).

**Files touched.** `crates/bt-core/src/types.rs` (`BudgetConfig`),
`crates/bt-core/src/events.rs` (`BudgetConfigured` variant),
`crates/bt-session/src/migration.rs` (new
`session_budget_projection` table),
`crates/bt-session/src/store.rs` (writer + reader for the projection),
`crates/bt-protocol/src/dto.rs` (budget fields on
`CreateSessionRequest` and new `UpdateSessionBudgetRequest`),
`crates/bt-protocol/tests/fixtures/session_control.json` (round-trip
fixtures).

**Depends on.** none (independent of 1.3*; touches different files
within the same hotspot crates, but coordinate with 1.3e and 1.4 which
also touch `bt-core` event/type defs and `bt-session`).

**Lane.** `hotspot:bt-core` + `hotspot:bt-session` + `hotspot:bt-protocol`
— three single-writer hotspots at once. Coordinate serial ordering
with 1.3e and 1.4.

**Fix.**

- Add `bt-core::BudgetConfig { max_wall_clock_seconds, max_tokens,
  max_turns, max_cost_usd }` with all fields optional.
- Document its relationship to `ThinkingConfig.budget_tokens`: session
  budgets are autonomy limits, not model-thinking budgets.
- Add a `session_budget_projection` table in `bt-session` keyed by
  `session_id`, rebuildable from the event log (`BudgetConfigured`
  events set it, `BudgetCheckpoint` events update the used-so-far
  counters).
- Add `EventPayload::BudgetConfigured` for initial and updated budget
  state.
- Add budget fields to `CreateSessionRequest` and a new
  `UpdateSessionBudgetRequest` in `bt-protocol`; round-trip fixtures
  added to the existing `session_control.json`.

**Invariant.** FR-12 (autonomous budgets first-class control).
Maintainability Principle 5 (data shapes first).

**Exit test.** Protocol fixture round-trip, session projection rebuild
test, new fixture entries in `session_control.json`.

#### 3.2 Budget enforcement with explicit runtime/agent split

**Problem.** The current system has neither restart-safe persisted budget
accounting in runtime nor a clean implementation of per-turn budget
checks flowing through the typed turn path reached from
`crates/bt-server/src/main.rs:1710`. `BudgetCheckpoint` events are never
emitted.

**Seam.** `bt-agent` for per-turn completion/budget semantics inside the
pure loop; `bt-runtime` for persisted budget state, restart-safe
accounting, and control-plane state transitions; `bt-server` remains
the thin transport entrypoint into that runtime path.

**Files touched.** `crates/bt-agent/src/lib.rs` (per-turn budget
evaluation), `crates/bt-runtime/src/runtime/sessions.rs` and
`crates/bt-runtime/src/runtime/budget.rs` (persisted counter reads,
`BudgetCheckpoint` emission, `SessionCancelled
{reason: "budget_exhausted"}` path), `crates/bt-server/src/main.rs`
around `:1710` (passes
configured budget into the turn driver; transport only).

**Depends on.** 3.1 (BudgetConfig + projection must exist before
enforcement can read from them).

**Lane.** `hotspot:bt-runtime`. Coordinate serial ordering with 4.2b
(turn-orchestration relocation) — 3.2 lands BEFORE 4.2b if both are
active in the same window, because 4.2b will move the code 3.2 adds.

**Fix.**

- Load the session's `BudgetConfig` at turn start.
- Load the latest used-so-far counters from the canonical budget
  projection.
- Compute wall-clock usage from durable event/projection state rather
  than a process-local `tokio::time::Instant` that resets on restart.
- Keep the budget split explicit: `bt-agent` decides turn-local
  budget/completion outcomes, while `bt-runtime` decides persisted
  session/control-plane consequences and records canonical events.
- Emit a `BudgetCheckpoint` event at turn boundaries and at configurable
  intra-turn intervals.
- When any maximum is exceeded, emit `SessionCancelled` with
  `reason: "budget_exhausted"`, cause the canonical
  `session_control_projection.cancel_requested` flag to flip, and halt
  further turn progression gracefully (do not kill an in-flight tool).
- Keep `bt-server` as transport only; do not add new budget semantics
  that bypass the runtime seam.

**Invariant.** FR-12. Fail-closed (Maintainability Principle 6).

**Exit test.** Integration test with `max_wall_clock_seconds = 3` and a
deliberately-long MockProvider, plus a restart-in-the-middle variant,
asserting the session halts at a turn boundary with the correct event
trail and restart-safe accounting.

#### 3.3 Extended OpenInference token attributes

**Problem.** `bt-otel::llm_span_attributes` emits only
`llm.token_count.prompt`, `llm.token_count.completion`, and
`llm.token_count.total`. Our `TokenUsage` already carries
`cache_read_tokens`, `cache_write_tokens`, `reasoning_tokens`, which
get dropped on export.

**Seam.** `bt-otel` (attribute builders), `bt-core::events::oi_attrs`
(constants).

**Files touched.** `crates/bt-core/src/events.rs` (`oi_attrs` module
gets new constants), `crates/bt-otel/src/lib.rs`
(`llm_span_attributes` emitter), test update in
`crates/bt-otel/tests/` or inline.

**Depends on.** none (TokenUsage already carries the source fields;
this is pure export-side work).

**Lane.** `hotspot:bt-otel` + `hotspot:bt-core` (small touch to the
oi_attrs module only). Coordinate with any simultaneous bt-core event
work.

**Fix.** Add constants and emit, when present:

- `llm.token_count.prompt_details.cache_read`
- `llm.token_count.prompt_details.cache_write`
- `llm.token_count.completion_details.reasoning`

**Invariant.** FR-5, IR-6.

**Exit test.** `bt-otel` test asserts extended attributes appear on
exported OTLP spans when `TokenUsage` has the relevant fields.

#### 3.4 Sacred Write Path invariant tests (split by seam)

**Problem.** The write-path rules are one invariant with three
distinct failure modes, each owned by a different crate. Lumping them
all in `bt-runtime` blurs the seams. The actual code layout:

- SQLite append lives in `crates/bt-session/src/store.rs:261`
  (`append_event`)
- Commit-then-broadcast sequencing lives in
  `crates/bt-runtime/src/runtime/records.rs` (`append_event`) — calls
  store first, then `event_bus.send`
- Turn halting on store failure belongs to the runtime-owned turn
  orchestration path reached through the server integration entrypoint
  in `crates/bt-server/src/main.rs:1710`

Each gets its test in the correct crate:

**Files touched.**
- 3.4a: `crates/bt-session/tests/` (new concurrency test) — may also
  need a small helper in `crates/bt-session/src/store.rs` for
  test-only access.
- 3.4b: `crates/bt-runtime/tests/` (new broadcast-ordering test with
  injectable store delay).
- 3.4c: `crates/bt-server/tests/` (new server integration test with a
  failing store stub).

**Depends on.** none for 3.4a and 3.4b. 3.4c depends on 3.4b landing
first so commit-before-broadcast is already guaranteed at the time of
the store-failure assertion.

**Lane.**
- 3.4a: `hotspot:bt-session` if it needs store helpers; otherwise
  `isolated` if it stays in a new test file.
- 3.4b: `hotspot:bt-runtime`; do not run while 3.2 is active.
- 3.4c: `hotspot:bt-server`; do not run while the `bt-server` portion
  of 4.2a or 4.2b is active.

These can be assigned to different writers only when the integrator has
confirmed the crate-level hotspots are not active elsewhere.

**Fix.** Add three seam-owned write-path invariant tests:

- 3.4a covers SQLite sequence allocation in `bt-session`.
- 3.4b covers commit-before-broadcast ordering in `bt-runtime`.
- 3.4c covers store-failure surfacing through the `bt-server`
  integration boundary without making `bt-server` the semantic owner.

**3.4a (`bt-session`) — `seq_id` monotonic under concurrency.**
N concurrent `append_event` calls on the same session; assert all
returned `seq_id` values are unique and strictly monotonic. This is
the store's responsibility.

**3.4b (`bt-runtime`) — commit-before-broadcast ordering.**
Subscribe to the runtime's broadcast channel, inject an observable
delay into the store, and assert no envelope appears on the broadcast
channel before the store commit returns `Ok`. This is the runtime
wrapper's responsibility (the path in
`crates/bt-runtime/src/runtime/records.rs`).

**3.4c (`bt-server`, integration) — store-failure halts the turn.**
Spawn a server with a store that returns an I/O error on append;
attempt a turn; assert the runtime-owned turn path terminates, emits a
`SessionError` event of class `Storage`, surfaces the failure through
the HTTP boundary, and does not silently continue. This test lives in
`bt-server` because it exercises transport surfacing around the runtime
seam at `crates/bt-server/src/main.rs:1710`; it does not make
`bt-server` the semantic owner of turn-halting behavior.

**Invariant.** The Sacred Write Path (architecture/overview.md,
event-taxonomy-and-write-path.md). Maintainability Principle 3 (single
source of truth per concern): the test lives where the behavior is
implemented.

**Exit test.** All three tests pass, each in its owning crate.

---

### Direction 4: Modularization and CI

CI is the highest-leverage missing foundation item. Modularization is
the second. Do these together so CI catches any regression introduced
during refactors.

#### 4.1 Stand up CI (do this first)

**Problem.** No `.github/workflows/`, no `rust-toolchain.toml`, no
`clippy.toml`. The `bt-agent` purity test and the stabilization fixtures
are enforced only by developer discipline.

**Seam.** Repo root. If the required `cargo clippy --workspace
--all-targets -- -D warnings` gate is already red on the pinned
toolchain, this task may include the smallest mechanical lint-baseline
cleanup needed to make the CI gate truthful. That cleanup is not a
feature slice and must not redesign subsystem APIs.

**Files touched.** `.github/workflows/ci.yml` (new),
`rust-toolchain.toml` (new), optionally `clippy.toml` (new),
`Makefile` (add `acceptance` target), optionally a small
`scripts/file_size_lint.sh` (new). If baseline clippy is red, also
touch the minimum source files required for mechanical lint cleanup and
record that widened write set in the execution tracker.

**Depends on.** none (landing this first is the plan's intent —
everything else benefits from CI being live).

**Lane.** `isolated` if the pinned clippy gate is already green.
Otherwise `hotspot:lint-baseline` until the mechanical cleanup lands;
do not run other writers against crates touched by that cleanup.

**Fix.**

- `.github/workflows/ci.yml` running on every push and PR:
  - `cargo fmt --all -- --check`
  - `cargo check --workspace --all-targets`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - `cargo test -p bt-agent --test purity`
  - a crate-boundary guard proving Belltower crates do not depend on
    app-specific Frollo crates or services
- `rust-toolchain.toml` pinning the compiler version supporting edition
  2024.
- A `make acceptance` target (only runs locally, not in CI by default)
  that runs scripted milestone tests from 4.4. **Initial target in
  4.1 is a stub** that prints
  `"no acceptance tests registered yet — populated by 4.4"` and exits
  with a non-zero status. This prevents a silent green pass during
  Weeks 1-4 before 4.4 wires the actual milestones. 4.4 replaces the
  stub with the real milestone invocations.
- If `cargo clippy --workspace --all-targets -- -D warnings` is red on
  the pinned toolchain before CI lands, fix only the mechanical lint
  baseline needed for the gate to pass. Prefer local equivalent rewrites
  and narrow `clippy.toml` threshold policy over broad crate-level
  allows. Do not mix architectural refactors into this cleanup.

**Invariant.** IR-10 (CI expectations), FR-19 (app and repo
independence), Maintainability Principle 2, and Maintainability
Principle 12.

**Exit test.** Locally, the exact CI commands above pass, the
crate-boundary guard passes, and `make acceptance` prints the required
stub message and exits non-zero. A pushed branch triggers CI; the
workflow fails on any check above.

#### 4.2 Decompose monolithic files (outcome-based) + relocate turn orchestration

**Problem.** Several files have exceeded the 1,500-line
working-context bound during foundation work. The current snapshot
above is the source of truth for which files still have explicit
override entries; the full 4.2 decomposition scope included:

- `crates/belltower/src/main.rs`
- `crates/bt-client/src/lib.rs`
- `crates/bt-providers/src/openai.rs`
- `crates/bt-server/src/main.rs`
- `crates/bt-runtime/src/runtime.rs`
- `crates/bt-session/src/store.rs`
- `crates/bt-tui/src/command_actions.rs`
- `crates/bt-tui/src/command_render.rs`

Compounding the size problem, **turn orchestration is currently in
`bt-server`, not `bt-runtime`**. The orchestration loop at
`crates/bt-server/src/main.rs:1710-1898`
(`run_session_turn_with_turn_id`) owns turn-id sequencing,
`TurnStartSource` decisions, settings-revision routing, and the
`PostTurnControlAction` matching that decides loop continuation. This
violates Principle 3 today and entangles the monolith split with a
semantic relocation.

**Seam.** The respective crates plus a new `bt-runtime::TurnOrchestrator`
abstraction that absorbs the orchestration logic out of `bt-server`.

**Files touched.**
- 4.2a: the override-listed monolith files named above and their new sibling
  modules. `docs/development/file-size-overrides.md` (new) if any
  file gets a documented override instead of a split.
- 4.2b: `crates/bt-server/src/main.rs` around
  `run_session_turn_with_turn_id:1710-1898` (shrinks to a transport
  entrypoint), a new module in `crates/bt-runtime/` (e.g.
  `crates/bt-runtime/src/turn_orchestrator.rs`),
  `crates/bt-runtime/src/lib.rs` (module export), and any
  `bt-runtime` public API surface needed by `bt-server` callbacks.

**Depends on.**
- 4.2a: depends on 4.1 (CI green before refactor lands so regressions
  are caught). The `bt-runtime` and `bt-server` portions of 4.2a also
  wait for 3.2 to land, because 3.2 edits the same turn-orchestration
  hotspots and is a behavior change rather than a pure refactor.
- 4.2b: depends on 5.4a-min and 5.4a-control (hard behavioral gates),
  plus 3.2 landing first if budget enforcement is still being
  threaded through the old path.

**Lane.**
- 4.2a: each monolith is its own single-writer `hotspot:<crate>` lane
  — one lane per override-listed crate/file. They may proceed in
  parallel only if each writer owns a distinct file set. The
  `bt-runtime` and `bt-server` 4.2a lanes are blocked while 3.2 or
  4.2b is active; TUI, provider, launcher, session, and client splits
  must likewise avoid any simultaneously active hotspot task in their
  owning crate.
- 4.2b: single writer on `hotspot:bt-runtime` + `hotspot:bt-server`
  simultaneously; do not parallelize with any other bt-runtime or
  bt-server task.

**Design before code (4.2b only).** Before the relocation PR opens,
the implementer must produce a 1-page interface sketch PR that
contains:
- the public surface of `TurnOrchestrator` (methods, input/output
  types, callback shape, error taxonomy);
- an enumeration of every current caller of
  `run_session_turn_with_turn_id` and how each will rewire;
- a statement of what stays in `bt-server` after the move (HTTP entry,
  auth, request parsing, response encoding) and what moves to
  `bt-runtime` (loop control, `TurnStartSource` resolution, settings-
  revision routing, `PostTurnControlAction` matching);
- a note on how 3.4b (commit-before-broadcast) will follow the
  relocated commit emission path if emission sites move.

The sketch lands first as a doc/design PR with review sign-off. Only
then does the relocation PR open. This is not optional — 4.2b without
a design sketch is high-risk because turn orchestration is the
plan's most semantically consequential move.

Current status: the design sketch has landed in
[`../development/turn-orchestrator-interface-sketch.md`](../development/turn-orchestrator-interface-sketch.md).
The initial relocation has also landed: `bt-runtime::TurnOrchestrator`
owns the turn loop, while `bt-server` remains a transport/adapters
consumer. Future runtime/server decomposition must still reserve the
`hotspot:bt-runtime` + `hotspot:bt-server` lane explicitly.

**Fix.** Outcome, not tree recipe — but the turn-orchestration
relocation is **explicitly NOT a pure refactor** and is sequenced
separately:

**4.2a — Pure-refactor monolith decomposition (no behavior change):**

- Every source file ends at most ~1,500 lines (test modules may be
  larger and can live in `tests/` instead of inline).
- Every module includes a one-paragraph header comment stating its
  scope and what lives elsewhere.
- Cross-module calls go through explicit public items; no reaching
  into sibling module privates.
- The refactor ships as **pure-refactor commits with no behavior
  change** — diff should show only moves, re-exports, and visibility
  adjustments.
- CI must be green (step 4.1) before the refactor lands so regressions
  are caught.
- All override-listed monoliths are in scope for future decomposition.
  Files not split yet must keep explicit override entries in
  `docs/development/file-size-overrides.md`; new oversized files fail
  CI unless they are intentionally added to that list.

**4.2b — Turn orchestration relocation (semantic move, gated by 5.4a):**

- Extract `bt-server::run_session_turn_with_turn_id` and its loop
  control logic into a new `bt-runtime::TurnOrchestrator::run_session_turns()`
  method.
- `bt-server` retains only the transport/HTTP entry point that calls
  the runtime orchestrator and wires callbacks.
- Loop control, `TurnStartSource` resolution, settings-revision
  routing, and `PostTurnControlAction` matching all move to runtime.
- This is a semantic relocation, not a pure refactor. **It must land
  AFTER 5.4a-min and 5.4a-control are green** (sequenced in Week 2-3
  alongside 3.1/3.2) so the inspection-reconstruction and
  control-persistence tests act as behavioral gates against silent
  semantics drift.
- 3.4b (commit-before-broadcast test) must still pass after the move;
  if commit emission grows in the relocated path, the test must be
  updated to follow it.

The exact module boundaries inside 4.2a are a design call by the
implementer; the plan does not prescribe them. Suggested starting
points live in the crate READMEs and in
[`../development/developer-guidelines.md`](../development/developer-guidelines.md).
For parallel execution, treat each override-listed monolith as its own
single-writer refactor lane. Do not split one oversized file with
multiple writers at once unless the integrator has already landed a
module map that makes the write sets disjoint. Do not mix behavior
changes into 4.2a branches; reserve 4.2b for the runtime relocation
explicitly.

**Invariant.** Maintainability Principle 1 (bounded working context),
Maintainability Principle 3 (single source of truth per concern — turn
orchestration belongs in `bt-runtime`).

**Exit test.** File-size lint in CI: a small helper script that fails
CI if any non-test `.rs` file exceeds 1,500 lines without an explicit
override entry in `docs/development/file-size-overrides.md`.
Plus: 3.4b (commit-before-broadcast), 5.4a-min (inspection
reconstruction), and 5.4a-control (cancel/steer survives restart) all
remain green after 4.2b lands.

#### 4.3 Extend TUI invariant coverage + operator-surface readability

**Problem.** `bt-tui` is already modularized and has real coverage in
`tests.rs` and `custom_terminal.rs`. The foundation bar required two
explicit coverage surfaces: (a) mechanical enforcement of specific
cutover invariants from
[`../development/codex-tui-cutover-note-20260407.md`](../development/codex-tui-cutover-note-20260407.md)
and (b) tests that assert operator-facing surface output (`/doctor`,
`/status`, `/model`,
`/inspect`, slash-command help, approval prompts) is *readable* —
not just syntactically correct. The TUI is the primary human
interface during foundation, so operator-surface quality is a
correctness concern, not a polish concern.

**Current status.** This coverage has landed. The coverage map lives in
[`../development/tui-invariant-coverage.md`](../development/tui-invariant-coverage.md);
operator fixtures live under `crates/bt-tui/tests/fixtures/operator/`;
and `cargo test -p bt-tui` runs the invariant and fixture tests.

**Seam.** `bt-tui` tests plus a small snapshot/golden-output corpus.

**Files touched.** `crates/bt-tui/src/tests.rs` (extend existing test
module with newly-covered invariants); new
`crates/bt-tui/tests/operator_surfaces.rs` (golden-output tests for
the operator-facing surfaces); new
`crates/bt-tui/tests/fixtures/operator/` directory for expected
renderings; `docs/development/tui-invariant-coverage.md` (audit note
listing each cutover invariant with its covering test).

**Depends on.** none.

**Lane.** `isolated` (bt-tui tests + fixtures only).

**Fix.** Two fix blocks in one task, both shippable as separate PRs:

**4.3a — Cutover invariant coverage.** Audit the cutover note's
invariants against existing tests; add tests only for invariants not
already covered. Likely missing:

- Streamed assistant text flushes on newline boundaries (partial lines
  stay in the active cell; completed lines move to scrollback).
- Viewport growth under composer expansion preserves scrollback (does
  not overwrite history rows).
- Slash-menu open/close transitions collapse the layout cleanly.

Do not rebuild coverage that already exists. Run a grep over
`tests.rs` first; add only the gaps.

**4.3b — Operator-surface readability tests.** Add golden-output
tests for the surfaces a human actually reads during dogfood:

- `/help` (or the slash-command discovery surface) — all foundation
  slash commands are listed, grouped, and include a one-line
  description each. Test pins the command set so silent additions
  show up as diffs.
- `/doctor` — the output for a representative mixed-connection
  configuration (one ready, one missing-auth, one runtime-unsupported,
  one configured-model-unavailable) is readable, labels each state
  clearly, and fits within 80 columns without wrapping pathologies.
- `/status` — the same connection matrix renders without information
  loss when compared to `/doctor` (i.e., `/status` is the compact
  sibling, not a disagreeing surface).
- `/model` — model selection UX renders the curated model list with
  current-selection indicator.
- `/inspect` — output for a representative session (reuses 5.4a-min's
  session builder) is readable; large outputs paginate or truncate
  deterministically; tool-call details readable.
- Approval prompt — when a tool requires approval, the prompt shows
  tool name + summarized arguments + approve/deny options within one
  screen.
- Error rendering — a canonical `SessionError` event renders with
  error class, code, and operator-actionable message (not just a
  stack trace).

Each golden-output test captures the rendered output for the scenario
and asserts against a committed fixture. Fixture drift fails CI;
intentional UX changes require explicit fixture updates.

**Invariant.** Codex TUI cutover invariants (4.3a). Principle 7
(telemetry is not optional — the operator's view of session truth is
a telemetry surface) and FR-16 (strong operator UX) for 4.3b.

**Exit test.** Missing invariants have covering tests, and the audit
note in `docs/development/tui-invariant-coverage.md` lists the test
that covers each cutover invariant; no uncovered invariants remain.

#### 4.4 Script target milestones by their real IDs

**Current status.** An initial deterministic runner exists at
`scripts/acceptance.sh` and is wired through `make acceptance`. It
executes non-live milestone proxies and can run the live local/cloud
provider lanes when `BELLTOWER_LIVE_ACCEPTANCE=1` is set. The live
M1/M2 provider lanes and M4.1 500-turn TUI stress gate passed locally
on 2026-04-24; evidence is recorded in
`docs/development/foundation-acceptance-log-20260424.md`. Remaining
operational work is the M9 dogfood gate.

**Problem.** Milestones M0–M9 in
[`./target-implementation-goals.md`](./target-implementation-goals.md)
are manual acceptance gates. None are scripted.

**Seam.** Workspace-level acceptance harness (a new `tests/acceptance/`
structure or per-crate `#[ignore]` tests grouped under a
`make acceptance` target).

**Files touched.** New `tests/acceptance/` directory tree at workspace
root OR new `#[ignore]` tests under each crate's `tests/` directory
(`crates/bt-server/tests/`, `crates/bt-mcp/tests/`, etc.). Updates to
`Makefile` to wire `make acceptance`.

**Depends on.** 4.1 (`make acceptance` target definition).
Milestone-by-milestone dependencies: M0 no deps; M1 depends on a
local backend; M2 depends on 1.1, 1.2, 1.3e for truthful cost; M3
depends on 3.1/3.2; M5 depends on session-lineage surfaces
stabilized; M7 depends on MCP happy path which is already present;
M7.1 depends on 1.4; M8 depends on 3.3.

**Lane.** Each milestone script is its own `isolated` lane — they do
not collide with each other or with hot-path crate work, since they
run against the built artifacts, not against in-flight source files.

**Fix.** Script the milestones that can honestly run in CI-adjacent
infrastructure. Use the milestone IDs and meanings from
`target-implementation-goals.md` directly. The plan commits to
scripting:

- **M0**: install and health (`setup`, `doctor`, `status`, launcher
  startup).
- **M1**: local model happy path.
- **M2**: supported cloud-provider happy path.
- **M2.1**: control-plane and protocol health, including typed errors
  and replay semantics, plus cancel/steer persistence across restart.
- **M3**: tool use and approvals.
- **M4**: session replay and raw chunks.
- **M5**: branching and tree continuity.
- **M6**: compaction correctness.
- **M7**: first-class MCP happy path.
- **M7.1**: instructions and skills acceptance.
- **M8**: telemetry/export, including OTLP validation against the
  existing live OTLP push test at `crates/bt-server/src/tests.rs`
  (`live_otlp_push_to_arize_preserves_session_correlation_across_turn_traces`).

`M4.1` remains the dedicated long-session/TUI gate in 5.6. `M9`
remains the dogfood gate in 5.7.

The live provider lanes are opt-in by design. Default CI must remain
deterministic; release candidates run:

```bash
BELLTOWER_LIVE_ACCEPTANCE=1 make acceptance
```

with `BELLTOWER_LIVE_LOCAL_CONNECTION` and
`BELLTOWER_LIVE_CLOUD_CONNECTION` available when the default `local`
and `chatgpt` choices are not the intended targets.

Tests marked `#[ignore]` by default; `make acceptance` runs them. Each
test describes explicitly what it does NOT cover (so the DoD stays
honest). Use one fixed header style in each milestone test file:

```rust
// Scope: install -> first turn against {provider}
// Not covered: network retries, non-happy-path errors, model fallback
```

**Invariant.** IR-9 (testability).

**Exit test.** `make acceptance` passes for the scripted milestone set
above against at least one cloud provider and the local Ollama backend;
each milestone test file documents its scope limits.

#### 4.5 Expand protocol fixtures

**Problem.** The fixture files at
`crates/bt-protocol/tests/fixtures/` are the public schema drift tripwire.
The foundation work needed them to cover the new budget/control DTOs,
instruction-provenance shapes, operator surfaces, export surfaces, and
stream envelopes introduced by the hardening work.

**Current status.** This coverage has landed. The fixture suite now
round-trips `session_control.json`, `inspection.json`,
`operator_surfaces.json`, `export.json`, and `stream.json` through
`crates/bt-protocol/tests/stabilization_fixtures.rs`. Budget/control
DTOs and instruction provenance are covered by the existing fixtures.
Vertex-specific readiness fixtures remain future work unless the Vertex
track is activated.

**Seam.** `bt-protocol` tests.

**Files touched.**
`crates/bt-protocol/tests/fixtures/session_control.json` (budget
round-trips from 3.1),
`crates/bt-protocol/tests/fixtures/inspection.json` or
`session_control.json` (instruction provenance from 1.4 if DTOs
grow), `operator_surfaces.json`, `export.json`, `stream.json`, and
optionally a Vertex readiness fixture if 2.3 lands.

**Depends on.** 3.1 (budget DTOs must exist in bt-protocol before
fixtures can serialize them), 1.4 (instruction-provenance DTOs, if
exposed on the wire), and 2.3 (Vertex readiness states, if the
Vertex track runs).

**Lane.** `hotspot:bt-protocol`. Serialize against any other
bt-protocol work in flight (5.1 OpenAPI generation).

**Fix.**

- Add or update public fixture files for the DTOs introduced by this
  plan: budget/control DTOs from 3.1, instruction-provenance DTOs from
  1.4 if exposed on the wire, and Vertex readiness states if 2.3 lands.
- Keep fixtures aligned with `bt-protocol` public shapes only; do not
  encode runtime-private fields.
- Add round-trip assertions so fixture drift fails in tests instead of
  surfacing as client breakage.

**Invariant.** FR-18 (protocol compatibility).

**Exit test.** Every DTO touched by this plan has at least one
round-trip assertion in the fixture suite.

---

### Direction 5: Consumer Surface and Dogfooding

The HTTP/SSE boundary already exposes the primitives external UIs
need: `/server/info` returns a rich `ServerCapabilities` struct
(`crates/bt-server/src/main.rs:2599`), `ErrorEnvelope` is returned from
bearer-auth and 404 paths (`crates/bt-server/src/main.rs:2539`, `:2700`), and
same-process SSE `Last-Event-ID` replay under broadcast lag is tested
(`crates/bt-server/src/main.rs:5856`). This direction **extends** that
coverage — it does not invent it.

This direction is also where the foundation makes session inspection
truth explicit. The operator-facing moat is not only that Belltower can
export telemetry later; it is that a human can inspect one canonical
session and recover the truthful story of turns, tool activity,
approvals/asks, raw provider behavior, cost/budget state, errors, and
lineage from store-backed surfaces.

Tool semantics are part of that same consumer surface. Built-in tools
and MCP tools remain distinct extension sources, but once they enter the
runtime they should obey the same harness contract: same canonical event
path, same approval/control path, same inspection model, same export
expectations, and truthful degraded/error reporting.

The gaps this direction targeted were:

- no TUI long-session smoke (500+ turn autonomous load)
- no scripted interactive operator-scenario walk through the TUI
  (login → doctor → model → first turn → approval → inspect → compact
  → export) — the surface a human actually uses during dogfood
- no dogfood log

The first two are now wired through the deterministic and tmux TUI
acceptance paths described in 5.6. The dogfood log remains the final
operational gate in 5.7.

#### 5.1 OpenAPI generation (shared ownership)

**Problem.** There is no committed OpenAPI spec. Carillon (or any
codegen consumer) must walk Rust types manually.

**Seam.** Shared between `bt-server` (route metadata, authorization,
paths) and `bt-protocol` (DTO schemas). Neither crate owns the full
contract alone.

**Files touched.** `crates/bt-protocol/src/dto.rs` (schemars
derivation attributes or equivalent behind feature flag),
`crates/bt-protocol/Cargo.toml` (feature flag), new
`crates/bt-server/src/openapi.rs` module (walker + emitter),
`crates/bt-server/Cargo.toml`, `xtask/` or `Makefile` (target that
regenerates `docs/api/openapi.yaml`),
`docs/api/openapi.yaml` (new, committed), and CI workflow step that
regenerates and diffs.

**Depends on.** 4.1 (CI must be live so the diff check is enforced),
4.5 (protocol fixtures from this plan must be DTOs OpenAPI can
derive against).

**Lane.** `hotspot:bt-protocol` + `hotspot:bt-server`. Serialize with
4.5 fixture work.

**Fix.**

- Annotate DTOs in `bt-protocol` with a schema derivation mechanism
  (e.g., `schemars` behind a feature flag) so each DTO knows its JSON
  schema.
- Add a small `openapi` module in `bt-server` that walks the route
  table and emits an OpenAPI 3.1 document, pulling DTO schemas from
  `bt-protocol`.
- A `cargo xtask openapi` or `make openapi` target regenerates
  `docs/api/openapi.yaml`.
- CI runs the generator and fails if the committed file is out of date.

**Invariant.** FR-7 (server-first protocol with consumable contract).

**Exit test.** CI task: regenerate OpenAPI, diff against committed
version, fail if different. `docs/api/openapi.yaml` committed.

#### 5.2 Freeze the public `bt-client` surface

**Problem.** `bt-client::lib.rs` has many public methods. Some are
intended for remote-client consumption; some are launcher-internal
helpers. The distinction is not documented at the API surface level,
and this work should extend the existing `bt-client` control-surface
tests rather than duplicate them.

**Seam.** `bt-client`.

**Files touched.** `crates/bt-client/src/lib.rs` (doc annotations on
every public method; visibility tightening for `Internal` items),
extend the existing `bt-client` release-candidate control-surface test
(do not duplicate it).

**Depends on.** none structurally, but land after 5.1 if OpenAPI
generation surfaces any `bt-client` method that should be `Internal`.

**Lane.** `hotspot:bt-client`.

**Fix.**

- Use `consumer-modes.md` as the authority: annotate each public method
  on `BelltowerClient` with one of `/// # Remote client-safe`,
  `/// # Launcher / local TUI only`, or
  `/// # Internal (do not use)`.
- Move anything tagged `Internal` behind `pub(crate)` or
  `#[doc(hidden)]`.
- `cargo doc -p bt-client` generates documentation that makes the
  contract obvious.
- Extend the existing `bt-client` release-candidate control-surface test
  instead of creating a second overlapping client-contract harness.

**Invariant.** Smallest honest surface (Maintainability Principle 4).

**Exit test.** Every public method on `BelltowerClient` carries an
annotation line; a new doc test or lint enforces this.

#### 5.3 Package-local reference example consumer

**Problem.** There is no minimal end-to-end example of using
`bt-client` from outside the workspace. The workspace root is not a
package, so a repo-root `examples/` won't link cleanly.

**Seam.** `crates/bt-client/examples/`.

**Files touched.** `crates/bt-client/examples/reference_client.rs`
(new, ~100 lines), `crates/bt-client/Cargo.toml` (example registration
if needed), `docs/examples/reference-client.md` (new,
run instructions).

**Depends on.** 5.2 (`bt-client` surface annotated so the example
uses only `Remote client-safe` methods), 5.5 optional (if SSE
reconnection behavior is exercised in the example).

**Lane.** `isolated` (example + doc only).

**Fix.** Add `crates/bt-client/examples/reference_client.rs` (~100
lines):

- `BelltowerClient::new_with_auth_token` against a local `bt-server`.
- `GET /server/info` first; negotiate capabilities before creating a
  session.
- `POST /sessions` to create.
- `POST /sessions/{id}/message` to send a user turn.
- Subscribe to SSE; stream events.
- On a tool call that requires approval, approve it via
  `POST /sessions/{id}/approve`.
- On `TurnFinished`, export JSONL and print a summary.

A short `docs/examples/reference-client.md` explains how to run it
against a locally-running `bt-server`.

**Invariant.** FR-7, IR-9.

**Exit test.** `cargo run --example reference_client -p bt-client`
works against a live local `bt-server` end-to-end. This example is run
as part of `make acceptance`.

#### 5.4 Error envelope contract matrix

**Problem.** Individual error-envelope paths are tested at
`crates/bt-server/src/main.rs:2539` (bearer-auth failure) and `:2700`
(404-with-typed-envelope). No single test covers the full surface
matrix, and no `bt-client`-side test asserts clients see
`ErrorEnvelope` across every error class.

**Seam.** `bt-server` protocol test using the existing in-crate server
harness, with `bt-client` used for the scenarios a public client can
express directly.

**Files touched.** `crates/bt-server/src/main.rs` for the matrix test
and malformed-JSON protocol handling; `crates/bt-server/Cargo.toml`
only if the server-local extractor needs an explicit dependency.

**Depends on.** none (builds on already-present error envelope paths).

**Lane.** `isolated` (single `bt-server` protocol-contract test plus
server-local malformed-request handling).

**Fix.** Drive `bt-server` through a representative error matrix from
the client side: bad auth, missing session, malformed request,
protocol-version mismatch, storage-unavailable simulation. Assert each
response body deserializes as `ErrorEnvelope` using the public
`ErrorClass` taxonomy that exists today, and record the actual
class-per-scenario mapping so future regressions show up as mapping
drift instead of assuming a class in advance.

**Invariant.** IR-11.

**Exit test.** The new protocol-contract test asserts that **every** scenario
in the matrix (bad auth, missing session, malformed request, protocol-
version mismatch, storage-unavailable simulation) produces a response
body that deserializes as `ErrorEnvelope` with a non-default
`ErrorClass`. The test also records the observed
scenario→`ErrorClass` mapping in a comment block at the top of the
file so future changes that silently remap scenarios cause a visible
diff.

#### 5.4a Canonical inspection truth and shared tool semantics

**Problem.** The architecture and subsystem docs treat canonical
inspection as the operator truth surface, and they require MCP tools to
flow through the same runtime path as built-ins. Today the plan covers
pieces of that through milestones and subsystem-specific checks, but it
does not yet require one explicit contract test proving a representative
session can be reconstructed coherently from store-backed inspection
surfaces rather than transcript heuristics or tool-source-specific
logic.

**Seam.** `bt-session` projections and read paths, `bt-runtime`
inspection/query services, `bt-protocol` inspection DTOs,
`bt-client` integration tests, and `bt-mcp` only where MCP inventory or
tool-origin details must be surfaced canonically.

**Files touched.**
- 5.4a-min: new integration test under `crates/bt-client/tests/` (e.g.
  `inspection_contract_min.rs`), plus the existing `fake_mcp_server`
  fixture binary (already in `crates/bt-mcp/src/bin/`).
- 5.4a-control: new integration test under `crates/bt-client/tests/`
  (e.g. `control_persistence.rs`).
- 5.4a-ext: additional test files in the same directory, or extensions
  of the min harness.

**Depends on.**
- 5.4a-min: none structurally — exercises surfaces that already exist.
  Recommended to land after 5.3 (reference example consumer) so the
  test reuses the same bt-client setup pattern.
- 5.4a-control: depends only on existing control and inspection
  surfaces. It must not wait for 3.1 budget work unless execution
  discovers a concrete missing DTO in the control inspection surface.
- 5.4a-ext: depends on 3.1/3.2 for cost/budget assertions, 1.4 for
  instruction-provenance assertions.

**Lane.**
- 5.4a-min: `isolated` (new test file).
- 5.4a-control: `isolated` (new test file).
- 5.4a-ext: `isolated` (new test file or follow-on PRs).

**Fix.** Split canonical inspection coverage into three reviewable
test slices:

- 5.4a-min proves a representative session can be reconstructed from
  canonical inspection surfaces, including operator commands and both
  built-in and MCP tool activity.
- 5.4a-control proves cancel and steer control state survive restart
  through durable events/projections and inspection.
- 5.4a-ext adds later coverage for cost/budget, instruction
  provenance, additional MCP behavior, and lineage after their
  prerequisite shapes exist.

**Design before code (5.4a-min only).** Before the min harness PR
opens, the implementer must produce a short design note (half a page
is enough) that lists:
- the exact sequence of operations the test performs to build the
  representative session;
- which inspection surface each assertion hits (endpoint name plus
  projection or event-log source);
- the specific parity assertions between built-in tool and MCP tool
  rendering that prove Principle 10 holds at the test level.
The note lands as a comment at the top of the test file or as a short
PR description. This prevents the min harness from drifting into
"test the happy path and hope" territory given its size (~200 LOC).

This task is split into a minimum reviewable slice and an extended
coverage slice. Do not land it as one giant integration test.

**5.4a-min Fix.** Add a minimal inspection-contract harness that
creates or replays a session containing:

- normal message/turn flow
- at least one durable operator command or slash-command result,
  issued through the protocol/client path rather than injected directly
  into storage
- at least one built-in tool call and result
- **at least one MCP tool call and result** (use the existing
  `fake_mcp_server` fixture from `crates/bt-mcp/src/bin/`) — promoted
  into the minimum slice because Principle 10's shared-contract is
  load-bearing for `/improve` and skills, and the cheapest way to
  prevent silent divergence is to require both tool sources in the
  reconstruction test from day one
- one approval or `ask` pause/resume path
- raw streamed provider output
- at least one canonical error or degraded-path record

Assert that canonical inspection surfaces can reconstruct that session
truthfully without transcript heuristics, and that built-in and MCP
tool activity render under the same canonical event/approval/inspection
shape (no source-specific transcript scanning). Assert separately that
the durable operator command appears in inspection and in exportable
session history, not only in local UI output.

**5.4a-control Fix.** Add a focused sibling test for runtime control
persistence:

- Start a turn or resumed turn against a durable temp store.
- Issue cancel and steer through the protocol/client path, not by
  mutating runtime internals.
- Restart the server/runtime against the same store.
- Assert inspection reports the canonical control state and event trail
  without relying on transcript rescans.

**Asymmetry to acknowledge in the test.** Cancel is fully
projection-backed: `SessionCancelled` and `SessionCancelCleared` events
update `session_control_projection.cancel_requested` and survive
restart through the projection alone. Steer is **only durable in the
event log** today — no `session_steer_projection` exists; steer state
must be re-derived from `SessionSteered` and `SessionSteersResolved`
events. The test must:

- Assert cancel state reads correctly from the projection after
  restart (fast path; no event-log scan required).
- Assert steer state reconstructs correctly from the event log after
  restart (slow path; explicit event-log walk acceptable).
- If `session_steer_projection` is added later (out of scope for this
  plan), the test should be tightened to require projection-backed
  steer reads as well.

This backs the DoD requirement that cancel and steer survive restart,
honestly, with the asymmetry named. It is the direct behavioral backing
task for DoD #2 and for the M2.1 acceptance coverage below.

**5.4a-ext Fix.** Extend the same harness, or add tightly scoped sibling
tests, to cover:

- cost/budget state after 3.1 and 3.2 land
- instruction provenance after 1.4 lands if inspection surfaces expose it
- lineage or related-session context where the current protocol surface
  already exposes it
- additional MCP coverage beyond the single tool from 5.4a-min:
  multiple MCP servers concurrently, MCP server lifecycle transitions
  (configured → discovered → ready → degraded), MCP tool denial,
  MCP-driven approval reuse

For the shared tool contract, use explicit assertions for each axis:

- **Event path:** built-in and MCP tool activity appears as canonical
  stored events with comparable turn/tool identity.
- **Approval/control path:** approval, denial, pending, cancel, or
  degraded control state is represented by the same runtime inspection
  model once the tool is admitted.
- **Operator-command path:** slash/operator commands that affect
  session behavior or operator-visible truth are recorded as canonical
  named operations and appear in inspection/export surfaces.
- **Inspection model:** client-facing inspection can render built-in and
  MCP tool calls/results without source-specific transcript scanning.
- **Export semantics:** bundle/derived export surfaces include both
  built-in and MCP activity from canonical session state.
- **Error/degraded reporting:** source-specific failures remain
  truthful but use the same operator-visible degraded/error vocabulary
  rather than a hidden MCP-only path.

Differences should remain limited to transport, inventory, lifecycle, or
origin metadata, not execution semantics.

**Invariant.** Maintainability Principle 7 (telemetry is not optional),
Maintainability Principle 10 (one runtime tool contract),
Maintainability Principle 11 (named operations, no hidden side
effects), FR-13a (session lineage and inspectability), FR-14
(first-class MCP), and IR-6 (observability by construction).

**Exit test.** The minimum inspection-contract harness passes first;
the control-persistence sibling test passes before the DoD can be
claimed; extended MCP/lineage/budget assertions land as follow-on tests
rather than inflating the minimum PR. Update
`docs/subsystems/session-runtime-and-agent.md` to name session
inspection as canonical operator truth, and update
`docs/subsystems/tools-context-and-approvals.md` / `docs/subsystems/mcp.md`
if the shared tool contract wording changes.

#### 5.5 Cross-process SSE reconnection test

**Problem.** Same-process `Last-Event-ID` replay under broadcast lag
is already tested at `crates/bt-server/src/main.rs:5856`
(`event_stream_recovers_from_broadcast_lag_by_replaying_store_events`).
The cross-process case — a fresh `bt-server` process replaying events
written by a previous process — is not covered. This is the case that
matters for real client restart semantics.

**Seam.** `bt-server` restart/SSE protocol test using the existing
in-crate test harness.

**Files touched.** Existing `bt-server` test module in
`crates/bt-server/src/main.rs`, possibly a small helper if the current
restart/SSE helpers need one additional reusable primitive.

**Depends on.** none.

**Lane.** `isolated` (single `bt-server` test-slice; no runtime/server
behavior change).

**Fix.** Integration test that:

- Starts `bt-server` against a temp SQLite path, creates a session,
  writes N events, captures the last SSE envelope `id` / `seq_id`.
- Shuts the server down cleanly.
- Starts a new `bt-server` process against the same SQLite path.
- Opens SSE with `Last-Event-ID` set to the captured numeric cursor.
- Asserts: no gaps, no duplicates, new events written after restart
  appear correctly.

**Invariant.** FR-7 replay semantics, IR-6.

**Exit test.** The new integration test asserts, across a full
restart cycle: (a) the SSE stream opened against the second process
with the captured cursor receives zero events with `id` ≤ cursor,
(b) every event written between cursor and shutdown is replayed
exactly once, (c) new events written after restart append with
strictly monotonic `id` values that follow the pre-shutdown tail.

#### 5.6 TUI long-session smoke + interactive operator scenario

**Problem.** The inline TUI handles short sessions well; two distinct
failure surfaces needed explicit scripted coverage:

- Scale failures under 500+ turn autonomous load (scrollback, compact,
  inspect rendering, SSE reconnect).
- First-contact operator-flow failures that only show up when a human
  walks the normal path (login → doctor → model select → first turn
  → approval → inspect → compact → export).

The autonomous smoke catches the first class but not the second. The
second class is what actually blocks the operator during Direction 5.7
dogfooding — and it is the class that a human using the TUI as their
only interface is most likely to hit.

**Current status.** This coverage has landed. The default
`make acceptance` path runs the deterministic TUI gate through
`scripts/tui_acceptance.sh deterministic`. The opt-in full-screen path
uses `BELLTOWER_TUI_ACCEPTANCE_TMUX=1 make acceptance`, which drives the
scripted tmux operator walk. The M4.1 stress variant uses
`BELLTOWER_TUI_ACCEPTANCE_TMUX=1 BELLTOWER_TUI_ACCEPTANCE_TURNS=500
make acceptance`; the 2026-04-24 local run is recorded in
`docs/development/foundation-acceptance-log-20260424.md`.

**Seam.** `docs/development/tui-testing.md` + `make acceptance`.

**Files touched.** `docs/development/tui-testing.md` (extended with
both the scripted long-session test and the scripted operator-flow
walk), possibly new helper scripts under `scripts/` for driving the
tmux session, `Makefile` (wire both gates into `make acceptance`).

**Depends on.** 4.3 (TUI invariant coverage + operator-surface
readability audit so the smoke tests exercise the invariants and
surfaces the audit named). 5.4a-min (representative-session builder
is reused by the operator-flow walk's export step).

**Lane.** `isolated` (docs + scripts only; no crate-level code).

**Fix.** Two scripted gate layers, both driven from `tui-testing.md`.
The default `make acceptance` path runs the deterministic TUI gate
because it is CI-safe and does not require a real terminal multiplexer.
The full-screen tmux gate is version-controlled and runs through
`BELLTOWER_TUI_ACCEPTANCE_TMUX=1 make acceptance` so developers and
release operators can collect real terminal evidence without making
every CI run depend on tmux and local loopback process orchestration.

**5.6a — Long-session autonomous smoke.** Use the scripted tmux recipe:

- Drive a scripted session via a local OpenAI-compatible mock provider.
- The fast developer default uses a short turn count; the M4.1 stress
  variant sets `BELLTOWER_TUI_ACCEPTANCE_TURNS=500`.
- Verify scrollback integrity, `/compact` works, `/inspect` renders
  large outputs without clipping, SSE reconnect after forced server
  restart resumes cleanly.

**5.6b — Interactive operator-scenario walk.** Add a scripted
end-to-end walk that mimics a first-contact operator using the TUI
as their only interface:

- Launch fresh (no prior session state); walk the first-run bootstrap.
- `/doctor` from empty state — assert readable output.
- `/model` select and switch — assert the switch takes effect on
  the next turn and that the rendered status reflects it.
- Issue a first real turn (MockProvider backend is fine; the walk is
  about the interactive surface, not the provider).
- Trigger an approval prompt mid-turn — assert the prompt is
  discoverable, the accept/deny affordances are visible, and the
  outcome is reflected in the transcript.
- `/inspect` on the turn just executed — assert canonical state
  renders (no transcript heuristics).
- `/compact` the session — assert transcript survives, seq_id order
  preserved, no data loss.
- Export (bundle or JSONL) — assert the export completes and the
  export path is printed.

The walk is scripted (tmux-driven key sequences) so it runs headless
when `BELLTOWER_TUI_ACCEPTANCE_TMUX=1` is set; it is not a manual
checklist. The deterministic acceptance path remains the default CI
gate for TUI invariants and canonical inspection/export consumers.

**Invariant.** 5.6a: Codex TUI cutover invariants hold under load.
5.6b: FR-16 (strong operator UX) — the first-contact operator can
complete a full session without hitting a surface that is empty,
ambiguous, or silently broken.

**Exit test.** `make acceptance` runs the deterministic TUI gate.
`BELLTOWER_TUI_ACCEPTANCE_TMUX=1 make acceptance` runs the full-screen
operator walk, and
`BELLTOWER_TUI_ACCEPTANCE_TMUX=1 BELLTOWER_TUI_ACCEPTANCE_TURNS=500
make acceptance` is the M4.1 stress invocation. The acceptance logs in
`tui-testing.md` are supporting evidence, not substitutes for the
scripted gates. The operator walk's script is version-controlled so it
can be re-run against future TUI changes to catch operator-flow
regressions.

#### 5.7 Dogfood week

**Problem.** IR-15 and milestone M9 require Belltower to be used on
its own repo for real work. This has not happened.

**Seam.** Operator process; `docs/development/`.

**Files touched.** New dated log entry under `docs/development/` (e.g.
`docs/development/dogfood-log-YYYYMMDD.md`), plus whatever GitHub
issues the week surfaces.

**Depends on.** All other DoD items green. This is the last gate.

**Lane.** `isolated` (docs only); this is an operational gate, not a
code change.

**Fix.** One operator week (5 business days) using Belltower on the
Belltower repo for real coding tasks. Friction logged as issues but not
fixed during the week unless blocking. At week end, triage; prioritize
foundation gaps exposed before any post-foundation work starts.

"Blocking" is defined as: the operator cannot complete the intended
coding task without an out-of-scope feature (memory, subagents,
datasets, improve, research, ACP) or without a protocol/runtime
regression. Friction that slows work but does not prevent completion
is logged, not fixed.

**Invariant.** IR-15, M9, FR-16 (strong operator UX — the TUI is the
only interface during dogfood, so its friction surface is part of the
foundation gate, not a later concern).

**Exit test.** A dated log entry under `docs/development/` that
contains:
- the dates worked and the provider/model used;
- at minimum **5 distinct real coding tasks attempted** on the
  Belltower repo (not toy prompts), one line each describing task
  and outcome (completed / completed-with-friction / blocked);
- a friction issue list with GitHub issue links, each tagged with the
  foundation direction it relates to (or "post-foundation" if the
  friction is out of scope);
- an explicit **TUI friction section** with one line per dimension
  below, even if the line says "no friction observed":
  - approval prompt UX (discoverability of accept/deny, mid-turn
    interruption handling, prompt re-entry after scrollback);
  - long tool-output rendering (truncation behavior, scrollback
    navigation, `/inspect` fidelity on oversized outputs);
  - slash-command discoverability during real work (not just
    `/help` — whether the operator remembered what was available
    without re-reading docs);
  - error rendering (whether provider errors, auth failures, and
    budget exhaustions were readable and actionable in-TUI);
  - session-control surfaces (`/compact`, `/cancel`, `/steer`,
    `/inspect`) as used mid-task, not in isolation;
  - first-contact bootstrap friction (was anything ambiguous on
    launch for a cold-start operator, even once);
- a one-sentence triage verdict: foundation ready to ship, foundation
  ready after N specific fixes, or foundation has a structural gap
  requiring re-plan.

The TUI friction section is mandatory because the TUI is the only
interface the operator has during foundation; silence on a dimension
is a useful signal ("no friction observed") but absence of that
dimension from the log is not.

#### 5.8 Export round-trip suite

**Problem.** DoD #6 (bundle fidelity), #8 (JSONL + ShareGPT truthful
derived views), and #9 (HTML renderability) each claim export-format
fidelity, but no dedicated task asserts the claims mechanically. The
export code in `bt-otel` is live and exercised in passing by other
tests, but a format-specific round-trip suite does not exist. Without
one, the three DoD bullets could silently pass on the basis of "the
export function did not panic" rather than "the round-trip preserves
the canonical story."

**Seam.** `bt-otel` (export functions), `bt-server` HTTP export
routes, and workspace-level acceptance via `make acceptance`; driven
from a protocol-facing test harness that requests exports through the
HTTP surface, not by calling `bt-otel` directly.

**Files touched.** Extend the existing `bt-server` in-crate protocol
export test or add a sibling protocol test that builds a representative
session via the same helpers used by 5.4a-min and requests each export
format through the server. Optional helper in `bt-otel` for
deterministic fixture generation. `Makefile` gets an
`acceptance-exports` sub-target callable from `make acceptance`.

**Depends on.** 5.4a-min (reuses its representative-session builder).
For bundle-fidelity tests: 1.2 (raw-chunk completeness) must be
green so the bundle's raw-linkage assertions hold.

**Lane.** `isolated` (new test file; reads the export surfaces, does
not modify `bt-otel` source beyond optional fixture helpers).

**Fix.** A single test file with four focused round-trip assertions,
one per format:

- **Bundle round-trip.** Export → parse back into session
  representation → assert every canonical event from the original
  session appears in the bundle with raw-chunk references intact.
  No chunk-lineage loss, no event reordering, no silent dropping of
  `session_control_projection` / `approval_projection` /
  `tool_run_projection` state.
- **JSONL derived view.** Export → parse as line-delimited JSON →
  assert every `EventEnvelope` is present, in `seq_id` order, and
  that derived fields (cost totals, token counts) agree with the
  source projections. No canonical event invented by the exporter;
  no canonical event silently dropped.
- **ShareGPT derived view.** Export → parse as ShareGPT conversation
  → assert message order matches canonical turn order and that each
  tool-call / tool-result pair is represented without transcript
  heuristics. Accept that ShareGPT is lossy (approval state, budget
  state, lineage may not appear) but document what IS lost in a
  comment block; the test pins the loss surface so future drift
  shows up as a diff.
- **HTML renderability.** Export a 500-turn session → parse the
  resulting HTML (tolerant parser is fine) → assert no broken
  tags, balanced DOM, scrollback-length output survives. This is
  layout smoke, not visual regression.

**Invariant.** FR-17 (export and analysis), Maintainability
Principle 7 (telemetry is not optional; inspection/export must
reflect canonical session state).

**Exit test.** `cargo test -p bt-client --test export_round_trip -- --ignored`
passes all four assertions. Test file documents its scope limits
using the header template from 4.4 (Scope / Not covered).

---

### Direction 6: Auth Storage Modes

**Current status.** Completed on the active foundation branch. The
subtasks below remain as the acceptance trace for the landed auth-store
stack.

Earlier snapshots identified auth storage as file-only under
`bt-auth`. Competitive harnesses (Codex, Hermes) offer an auth-storage
menu (file / keychain / ephemeral / auto) so operators on shared
machines, CI environments, or managed laptops have appropriate options.
Belltower now has the same foundation menu; future work should focus on
migration/import UX rather than another storage-seam redesign.

This direction is scoped tightly: refactor the existing store behind
a trait, add the two missing backends (keychain, ephemeral), and add
auto-selection with explicit precedence. Import flows (e.g., "import
credentials from `gcloud` or Claude Code") are out of scope and
belong to post-foundation.

#### 6.1 `CredentialStore` trait + file backend refactor

**Problem.** `bt-auth` currently couples `resolve` / `resolve_fresh` /
credential-read paths to a concrete file-on-disk implementation. A
new backend (keychain or ephemeral) requires threading
implementation-specific logic through the same paths. This is a
shape-first problem: the credential persistence backend needs to be
pluggable before any new backend can land cleanly.

**Seam.** `bt-auth`.

**Files touched.** `crates/bt-auth/src/lib.rs` (extract
`trait CredentialStore` with `load`/`store`/`delete`/`list` methods;
move existing file I/O into a `FileStore` impl of that trait; keep
public `CredentialResolver` API stable). New module structure may
land: `crates/bt-auth/src/store/mod.rs`, `store/file.rs`.

**Depends on.** none (does not share write-set with any active D1/D2
task; schedule before or after any other `bt-auth` work in the
Vertex track).

**Lane.** `hotspot:bt-auth`. Serialize against 2.2 if that track is
running.

**Fix.**

- Define `CredentialStore` trait in `bt-auth` with explicit methods
  matching current file-store behavior: load by connection id, store
  (upsert), delete, list all stored connections.
- Refactor existing file-based code into `FileStore` as the first impl.
- Make `CredentialResolver` hold `Arc<dyn CredentialStore>` rather
  than an inline file handle.
- No behavior change to external callers. All existing `bt-auth`
  tests must continue to pass without modification.
- Do **not** change the on-disk auth JSON format in this slice. If
  backend metadata is needed later, add it as a JSON schema/version
  field in a separate migration task, not as a non-JSON header.

**Invariant.** FR-3 (first-class auth), IR-5 (security by default).
Maintainability Principle 5 (data shapes before behavior).

**Exit test.** All existing `bt-auth` tests pass unchanged. A new
unit test exercises the `FileStore` directly through the trait
surface to confirm the trait contract is complete.

#### 6.2 Keychain and ephemeral backends

**Problem.** With 6.1's trait in place, Belltower still has only one
storage option. Shared-machine users (laptops with multiple logins)
need OS-native keychain storage; CI environments need ephemeral
env-var-only storage with no disk persistence.

**Seam.** `bt-auth`.

**Files touched.** New `crates/bt-auth/src/store/keychain.rs`
(macOS Keychain / Windows Credential Manager / Linux Secret Service
via the `keyring` crate or equivalent). New
`crates/bt-auth/src/store/ephemeral.rs` (reads env vars like
`BELLTOWER_OPENAI_API_KEY` at resolution time; never writes to disk;
never writes to keychain). `crates/bt-auth/Cargo.toml` (add keyring
dep behind a feature flag if platform support is partial).

**Depends on.** 6.1 (trait must exist first).

**Lane.** `hotspot:bt-auth`.

**Fix.**

- `KeychainStore` implements `CredentialStore` and stores each
  connection's credential under a per-connection keyring entry
  (service `belltower`, account `<connection_id>`).
- `EphemeralStore` implements `CredentialStore` with `store` and
  `delete` returning errors, `load` reading from env vars named by
  convention, and `list` enumerating env vars that match the
  convention.
- Selection and operator-facing reporting are deferred to 6.3, where
  the launcher/readiness surfaces are part of the declared seam.
- Cross-platform: keychain backend is feature-gated so builds on
  platforms without a supported keyring compile cleanly without it
  (fall back to file + warning).

**Invariant.** FR-3, IR-5, Principle 6 (fail closed — attempting to
write to ephemeral store errors visibly rather than silently
succeeding).

**Exit test.** New integration tests under `crates/bt-auth/tests/`
exercise round-trip for keychain (skipped on CI without keychain
access, gated by env) and ephemeral (pure env-var test, no
persistence).

#### 6.3 Auto mode with explicit precedence

**Problem.** Forcing users to pick a backend is a friction tax.
Without an auto mode, most users default to file storage even when
keychain is available. An explicit precedence chain picks the best
available backend at resolution time and documents its choice.

**Seam.** `bt-auth`, `belltower` (launcher output), `bt-readiness`
(doctor output).

**Files touched.** `crates/bt-auth/src/store/auto.rs` (new; selector
implementing `CredentialStore` by delegating to an ordered list of
backends). `crates/belltower/src/main.rs` (login `--storage`
selection, default store is `auto` unless overridden, and doctor/status
output includes active backend per connection). `crates/bt-readiness/`
if readiness DTOs or summaries need to expose auth-storage source
truth. Small update to `docs/development/consumer-modes.md` or a new
`docs/development/auth-storage.md` describing the precedence.

**Depends on.** 6.1, 6.2.

**Lane.** `hotspot:bt-auth` + `hotspot:belltower`.

**Fix.**

- `AutoStore` precedence on resolution:
  1. Environment variables (ephemeral) if
     `BELLTOWER_AUTH_STORE=ephemeral` or if an env var matching the
     expected convention is set for the requested connection.
  2. Keychain if a credential is stored there.
  3. File store as fallback.
- On `store`: writes go to the explicitly-configured backend
  (default keychain where available, else file). Never silently
  promotes a file-stored credential to keychain; migration is an
  explicit operator action (`belltower auth migrate --to keychain`).
- `belltower login --storage keychain|file|ephemeral|auto` selects
  the storage backend explicitly, and `BELLTOWER_AUTH_STORE` provides
  the same override for non-interactive use.
- `belltower doctor` reports: "credential for `<connection>` resolved
  from `<backend>` (tried: `<list>`)" so operators can verify.

**Invariant.** FR-3, IR-8 (honest capability reporting), Principle 7
(telemetry / inspection surfaces must reflect reality).

**Exit test.** Integration test proving precedence: set up a
connection with credentials in multiple backends, verify `resolve`
returns the highest-priority source; `doctor` reports the same. The
test also asserts the auto store's behavior with a missing backend
(e.g., keychain unavailable) falls through gracefully.

---

### Direction 7: Distribution

**Current status.** Initial distribution scaffolding is completed on the
active foundation branch: cargo-install docs, release workflow, and
publish preflight exist. Future release work should harden those gates
rather than recreate the scaffolding.

Foundation must ship something a dogfooder can share. Earlier
snapshots only had the workspace-checkout path
(`cargo run -p belltower`). The active branch now has the install docs
and release scaffolding needed for dogfood; remaining work is to run
and harden those release gates before the collaborator handoff in 5.7.

Scope is deliberately narrow: a source-checkout Cargo install path, a binary
release for the major platforms, and installation documentation. Public Rust
crate publication and package managers beyond Homebrew (apt, nix, winget) are
post-foundation.

#### 7.1 Source-checkout Cargo install path

**Decision.** `0.1.0` deliberately does not publish workspace crates to
crates.io. The registry already contains unrelated `belltower` and
`bt-runtime` packages; renaming the public package graph under release
pressure would create a premature compatibility surface. The supported Cargo
path is therefore installation from a Belltower source checkout. The installed
runtime still has a package-boundary seam: the primary operator command is
`belltower`, but the launcher executes `bt-server` and `bt-tui` as sibling
helpers, so the source path installs all three binaries.

**Seam.** Repo root plus each workspace Cargo.toml.

**Files touched.** `Cargo.toml` (workspace), and each
`crates/*/Cargo.toml` (version field, description, license, repository,
homepage, private-publication metadata, versioned internal path dependencies),
and `scripts/distribution.sh` for local-install verification. Release workflow
wiring owns the distribution and local-install checks.

**Depends on.** 4.1 (CI must be green so the released versions have
been validated).

**Lane.** `isolated` (all Cargo.toml edits; no code). Serialize
against any other Cargo.toml-touching work (e.g., 4.1's
`rust-toolchain.toml` + clippy.toml additions).

**Fix.**

- Ensure every crate has a version, description, inherited license,
  repository URL, homepage, and `publish = false`.
- Keep internal workspace dependencies path-based; versions remain useful for
  workspace coherence but do not promise registry resolution.
- The binary crates declare explicit `[[bin]]` entries for
  `belltower`, `bt-server`, and `bt-tui`.
- Verify the cargo-installed shape from local package paths: install
  `bt-server`, `bt-tui`, and `belltower` into a clean Cargo root, then
  verify the installed `belltower` binary starts.
- Treat any future crates.io package graph, including a possible single-package
  install, as a deliberate new public API decision.

**Invariant.** FR-19 (repo independence) — release archives and the
source-checkout install path must contain only Belltower code and public
dependencies; they must not pull any Frollo app code. The operator starts with
the `belltower` command even when helper binaries are installed alongside it.

**Exit test.** CI runs `scripts/distribution.sh --preflight` to verify that
every workspace crate remains private to the repository, then runs
`scripts/distribution.sh --install-check` to verify the local
`cargo install --path` shape and launcher helper discovery. Dogfood week
documents either the release archive or source-checkout path in the dogfood
log.

#### 7.2 Binary release artifacts

**Problem.** `cargo install` requires a Rust toolchain. Many users
(and dogfood collaborators) do not have one. A binary release that
runs out of the box on the major platforms is table stakes for
sharing a CLI tool.

**Seam.** Repo-root GitHub Actions workflow.

**Files touched.** New `.github/workflows/release.yml` (builds
macOS arm64 + x86_64, Linux x86_64 + aarch64, Windows x86_64 on
release-tagged pushes; uploads artifacts to the GitHub release).
`crates/belltower/src/main.rs` only if the CLI needs a `--version`
surface for release artifact smoke checks. Optional `.cargo/dist.toml`
if using `cargo-dist`.

**Depends on.** 4.1 (baseline CI must exist; release workflow reuses
the same toolchain / clippy / test gates before producing
artifacts).

**Lane.** `isolated` (new workflow; does not touch source crates).

**Fix.**

- Release workflow triggers on a pushed tag matching
  `v[0-9]+.[0-9]+.[0-9]+`.
- Builds the `belltower`, `bt-server`, and `bt-tui` binaries in
  release mode for all five platform targets. The helper binaries ship
  next to `belltower` so the installed launcher can start chat without
  requiring a workspace checkout.
- Runs the full CI matrix (fmt / check / clippy / test / purity /
  file-size / acceptance-min) before producing any artifact. A red
  CI aborts the release cleanly; no half-released artifacts.
- Uploads artifacts as `belltower-<version>-<target>.tar.gz` (Unix)
  or `.zip` (Windows). Each artifact ships `belltower`, `bt-server`,
  `bt-tui`, and a `README.txt` pointing to `docs/install.md`.
- Optional: sign the artifacts (cosign or ssh-agent) if the
  integrator has a signing key; otherwise skip signing for v0.1.

**Invariant.** FR-19 (repo independence), IR-10 (CI enforces
invariants — release artifacts are produced only from a fully green
CI run).

**Exit test.** Cut a test tag on a branch; verify the release
workflow produces all five platform artifacts; download one on a
clean machine and run `belltower --version` successfully.

#### 7.3 Installation documentation

**Problem.** The project README must describe how to install Belltower. New
users — especially dogfood collaborators — need a page that covers the two
supported install paths (release archive and source checkout) with a single
happy-path example per platform.

**Seam.** Repo-root docs.

**Files touched.** `README.md` (install section added or expanded).
New `docs/install.md` (detailed install paths + troubleshooting).

**Depends on.** 7.1 and 7.2 (docs describe paths that must actually
work).

**Lane.** `isolated` (docs only).

**Fix.**

- README.md install section: two supported options, one paragraph each:
  download a release archive or install from a source checkout.
- `docs/install.md` expands each path with platform-specific notes,
  common errors, and the `belltower doctor` first-run check.
- Cross-link from README to `docs/install.md`.
- Include a "Verify your install" section that runs
  `belltower --version`, `belltower doctor`, and `belltower status`
  and describes expected output.

**Invariant.** IR-14 (documentation expectations).

**Exit test.** A reviewer (or dogfood collaborator) follows
`docs/install.md` on a clean machine without prior workspace access
and reaches a running `belltower doctor` state. Dated entry in the
dogfood log confirms this.

---

### Direction 8: Web Retrieval Foundation

Web retrieval is no longer post-dogfood. It is important enough for
scientific-agent use that the foundation checkpoint needs a small,
canonical version before broader dogfooding begins.

Scope is deliberately narrow: two first-class built-in tools,
`web_search` and `web_fetch`, with stable request/response shapes and
backend selection behind the tool seam. This direction does **not**
thread provider-native hosted search through the model loop, does not
add browser automation, and does not add a separate `web_extract` tool
until real workloads justify the extra surface.

#### 8.1 Canonical web tool shapes

**Problem.** The previous `fetch` tool proved that URL retrieval belongs
in the canonical tool path, but it blurred search, fetch, extraction,
and provenance. Scientific workflows need search and fetch to be
inspectable, attributable, and backend-swappable without changing model
instructions.

**Seam.** `bt-core` owns request/response DTOs. `bt-tools` owns tool
implementation and backend selection. `bt-runtime`, `bt-session`, and
`bt-server` continue to treat these as ordinary tools flowing through
the same approval, event, inspection, and export path as every other
built-in tool.

**Files touched.** `crates/bt-core/src/types.rs`,
`crates/bt-tools/src/web.rs`, `crates/bt-tools/src/registry.rs`,
`crates/bt-context/src/lib.rs`, TUI tool-summary surfaces, and the
tool subsystem / architecture docs.

**Depends on.** Existing canonical tool event path and approval path.
No dependency on memory, browser automation, or provider-native hosted
search.

**Lane.** `hotspot:bt-tools`; serialize against other built-in tool
inventory edits.

**Fix.**

- Add `web_search` with query, optional `max_results`, optional `site`,
  and optional backend override.
- Add `web_fetch` with URL, optional format, and optional max-byte
  limit.
- Record backend, source URL/final URL, status, truncation, digest, and
  normalized body/result metadata in the tool result payload.
- Keep repository `search` distinct from external `web_search`.
- Keep direct browser automation out of this slice.

**Invariant.** Tool interfaces are foundational infrastructure:
canonical names and stable data shapes matter more than exposing every
backend immediately. All web retrieval remains visible through the
same durable tool telemetry as local tools.

**Exit test.** `bt-tools` tests prove the model-facing inventory exposes
`web_search`/`web_fetch` in extended mode, both tools are
approval-sensitive, private/local fetch targets are rejected before
network fetch, and backend responses normalize into canonical DTOs.

#### 8.2 First backend: Exa search plus direct HTTP fetch

**Problem.** Belltower needs a working web-research path without
locking itself to one vendor or building a large backend matrix before
dogfood.

**Seam.** Backend traits live inside `bt-tools`; backend configuration
lives in `bt-core` config; backend readiness lives in `bt-readiness`;
backend credentials resolve through the normal `bt-auth` resolver.
Additional providers implement the same search/fetch backend traits
and must not change the model-facing tool schema.

**Files touched.** `crates/bt-core/src/config.rs`,
`crates/bt-core/src/types.rs`, `crates/bt-auth`, `crates/bt-readiness`,
`crates/bt-tools/src/web.rs`, `crates/belltower/src/main.rs`, and docs
describing how to configure search backends.

**Depends on.** 8.1.

**Lane.** `hotspot:bt-tools`.

**Fix.**

- Add Exa as the first `web_search` backend, selectable through
  `[web].search_backend`, `BELLTOWER_WEB_SEARCH_BACKEND`, or an explicit
  tool backend argument.
- Store web backend credentials under synthetic provider ids such as
  `web:exa`, while preserving configured env sources such as
  `EXA_API_KEY` and `BELLTOWER_EXA_API_KEY`.
- Add `belltower web status`, `belltower web configure exa`, and
  `belltower web login exa` so setup is inspectable and matches the
  existing provider-auth operational pattern.
- Add direct HTTP as the first `web_fetch` backend with explicit
  public-URL safety checks and manual redirect validation.
- Report missing/unsupported backend configuration as a structured tool
  error rather than silently falling back to a different behavior.
- Document that Hermes-style provider expansion is welcome behind this
  seam: Brave, Tavily, SearXNG, Firecrawl, DDGS, and other providers
  can be added one backend at a time.

**Invariant.** Provider selection is a backend concern, not a new tool
taxonomy. Exa is the first provider, not the architecture.

**Exit test.** Unit tests cover Exa response normalization and direct
HTTP URL-safety checks without requiring live network access. A live
operator smoke can be added once an `EXA_API_KEY` is available.

---

## Sequencing

Dependencies are explicit. Parallel work is allowed only when the write
sets are disjoint or the integrator has assigned a single-writer lane.
The sequence below is a coordination plan, not a promise that every item
in a week can be edited at the same time.

**Week 0 (half day) — Establish the snapshot baseline:**

- Apply Direction 0 to the first touched crates on `main`: refresh the
  current snapshot claims, commit any drift corrections, and treat the
  same check as a standing pre-PR rule for later weeks.
- Create an execution tracker that records active branch/worktree,
  owner, task id, write set, reviewer/verifier, and merge order for
  each active slice.

**Week 1 — CI first, then correctness lanes:**

Current branch note: lanes A-D have landed on the active foundation
branch (1.1, 1.2, 1.3a-e, 1.4). They remain listed here to preserve
the original sequencing contract and to guide any future rework if a
Direction 0 snapshot shows drift.

- 4.1 CI pipeline (lands first; everything else benefits from it)
- Lane A, provider-only: 1.1 Anthropic reasoning tokens.
- Lane B, raw-provenance single writer: 1.2 Raw-chunk completeness,
  after 1.1 lands because both touch `crates/bt-providers/src/anthropic.rs`.
- Lane C, pricing/reprojection sequential stack: 1.3a -> 1.3b ->
  1.3c -> 1.3d -> 1.3e. Do not parallelize these across the same
  catalog, pricing, store, or inspection files. While Lane C is
  active, it exclusively owns the shared pricing/catalog/session-store
  hotspots those tasks touch.
- Lane D, instruction provenance: 1.4 can audit immediately, but code
  that depends on event-log walking waits for 1.3d's reusable harness.
  If 1.4 needs to edit `bt-core` event/type definitions or
  `bt-session` event-log walking code, it waits for the active 1.3*
  writer to clear those hotspots rather than running in parallel.
- Lane E, write-path invariant tests: 3.4a and 3.4b may proceed in
  parallel if their write sets stay inside their owning crates and
  avoid active runtime/server hotspots owned by other lanes. 3.4c
  starts only after 3.4b lands, even though its write set is disjoint,
  because the store-failure assertion depends on commit-before-broadcast
  already being pinned.

**Direction 6 (Auth Storage Modes)** runs as a single-writer stack on
`hotspot:bt-auth`: 6.1 → 6.2 → 6.3. May proceed in parallel with
most of the main sequence as long as no other `bt-auth` work is
active. Coordinate with 2.2 (Vertex ADC refresh) if the Vertex track
is also running — they share `bt-auth` as a hotspot. Target landing
window: Week 2–3 (alongside 3.1/3.2) so 6.3 is ready when dogfood
begins. Current branch note: 6.1-6.3 have landed; future auth work
should open a new task rather than reopening this stack.

**Direction 7 (Distribution)** is sequenced late because 7.1 requires
4.1 (CI) green, and 7.2 requires the release-ready CI matrix. 7.1 and
7.2 may run in parallel lanes (one writer on Cargo.toml edits, another
on the release workflow); 7.3 (docs) waits for both. Target landing
window: Week 4 for 7.1/7.2 and Week 5 for 7.3, so the dogfood week
(5.7) starts with a documented install path. Current branch note:
distribution scaffolding and install docs have landed; release-gate
hardening should be tracked as a follow-on if needed.

**Direction 8 (Web Retrieval Foundation)** runs before dogfood because
scientific-agent tasks need external evidence capture. It is a
single-writer `bt-tools` lane and should not run concurrently with any
other built-in tool inventory change. It may run after CI is green and
does not depend on the budget/runtime relocation stack.

**Week 2–3 — Budget + behavioral gates + pure-refactor decomposition + auth storage:**

The key sequencing change vs earlier drafts: **5.4a-min and 5.4a-control
land in this window, BEFORE 4.2b**. They act as behavioral gates
against silent semantics drift during the runtime relocation in 4.2b.

- 3.1 Canonical budget shape (must land before 3.2)
- 3.2 Budget enforcement in runtime turn orchestration
- 3.3 Extended OpenInference token attributes
- 6.1 `CredentialStore` trait + file backend refactor
- 6.2 Keychain and ephemeral backends (after 6.1)
- 6.3 Auto mode with explicit precedence (after 6.2)
- **5.4a-min** Canonical inspection-truth minimum harness — must
  include built-in + MCP tool calls per Principle 10
- **5.4a-control** Cancel/steer survives restart sibling test
- 4.2a Pure-refactor monolith decomposition, split by crate lane:
  any future override-listed files may proceed after their dependency
  gates if disjoint. `bt-runtime/runtime.rs` and `bt-server/main.rs`
  have already been split; future runtime/server file-size work still
  waits until any active semantic turn-orchestration work is green
  because those crates share the highest-risk hotspots. All 4.2a
  slices are strict no-behavior-change.
- 4.2b Turn-orchestration relocation from `bt-server` into
  `bt-runtime::TurnOrchestrator` — semantic move; requires 5.4a-min
  and 5.4a-control green as behavioral gates; do not begin until both
  pass
- 4.3 TUI invariant coverage audit + operator-surface readability
  (4.3a cutover invariants, 4.3b golden-output fixtures for `/help`,
  `/doctor`, `/status`, `/model`, `/inspect`, approval prompts, error
  rendering)

**Week 4 — Consumer surface + fixtures + extended inspection:**

- 4.5 Protocol fixture expansion (follows 3.1)
- 5.1 OpenAPI generation
- 5.2 `bt-client` surface freeze
- 5.3 Reference example consumer
- 5.4 Error envelope contract test
- 5.4a-ext Extended inspection coverage (cost/budget assertions after
  3.1/3.2, instruction-provenance assertions after 1.4, additional MCP
  coverage beyond the single tool from 5.4a-min, lineage where the
  current protocol surface exposes it)
- 5.5 SSE reconnection test
- 5.8 Export round-trip suite (bundle / JSONL / ShareGPT / HTML; reuses
  the 5.4a-min representative-session builder; depends on 1.2 for
  bundle raw-linkage assertions)
- 7.1 Source-checkout Cargo install path verified end-to-end
- 7.2 Binary release artifacts (parallel writer to 7.1)

**Week 5 — Scripted milestones + TUI smoke + install docs:**

- 4.4 Scripted target milestones by real IDs (initial deterministic
  runner is landed; live provider lanes remain follow-on)
- 5.6a TUI long-session autonomous smoke (`M4.1`)
- 5.6b TUI interactive operator-scenario walk (depends on 4.3b golden
  fixtures and 5.4a-min representative-session builder)
- 7.3 Installation documentation (depends on 7.1 and 7.2)

**Week 6 — Dogfood:**

- 5.7 Dogfood week

**Vertex track (Direction 2)** is optional and does not gate the
foundation DoD, but it is not conflict-free. If active, it needs an
explicit lane in the execution tracker because it touches catalog,
pricing, provider, readiness, auth, and launcher surfaces. Coordinate
2.1 with 1.3a/1.3b before editing shared catalog/pricing shapes.
Internally, **2.1 must land first**; 2.2 (ADC refresh policy), 2.3
(readiness transitions), and 2.4 (launcher UX) can then run behind it.
`2.2` can already be exercised through readiness before runtime
admission, but it still depends on the seam decision made in `2.1`.

## Definition of Done

This section defines the **foundation checkpoint**, not the entire final
release bar from `target-implementation-goals.md`.

The foundation checkpoint is reached when every one of these is true.
Each implemented invariant is machine-checked; operational gates also
carry dated evidence.

1. **Coherent operator story (FR-1).** `belltower setup`, `login`,
   `model`, `doctor`, and `status` tell one consistent story: the
   supported providers surfaced by `setup` match the ones `login`
   can authenticate, match the ones `model` can target, match the
   ones `doctor` and `status` report ready. Covers OpenAI, Anthropic,
   ChatGPT, and a local OpenAI-compatible backend. Validated primarily
   by M0; M1 and M2 confirm the local/cloud happy-path flows on top of
   that coherent command story. (Vertex optional via Direction 2.)
2. **Autonomous budget enforcement with control persistence
   (FR-12).** `belltower` runs autonomously with a configured budget
   (`max_wall_clock_seconds`, `max_tokens`, `max_turns`, `max_cost_usd`)
   and halts gracefully at any ceiling. Control persistence asymmetry
   is named honestly: **cancel** survives a mid-session process restart
   via `session_control_projection` (fast path, projection-backed);
   **steer** survives via the canonical event log
   (`SessionSteered` + `SessionSteersResolved` replay). A
   `session_steer_projection` is out of scope for this plan; if added
   later, this DoD bullet should be tightened to require
   projection-backed steer reads as well. The resulting session is
   replayable turn-by-turn. Validated by 3.2 plus the 5.4a-control
   persistence test and the M2.1 scripted acceptance coverage (both are
   hard gates, not follow-ons).
3. **Canonical inspection truth.** For a representative session, the
   operator can reconstruct the canonical story through session
   inspection surfaces: messages, durable operator commands, tool
   calls/results, approvals/asks, raw chunks, budget/cost state,
   errors, lineage, and MCP activity. This truth comes from canonical
   store-backed inspection rather than transcript heuristics. Built-in
   and MCP tools share the same runtime execution/inspection/export
   contract once admitted. **Backed by 5.4a-min, which must include at
   least one built-in tool AND one MCP tool end-to-end** (the cheapest
   way to prevent silent Principle 10 divergence as `/improve` and
   skills land later). The built-in tool coverage includes the
   canonical web retrieval seam from Direction 8: repository `search`
   stays local, while `web_search` and `web_fetch` produce durable,
   approval-sensitive tool telemetry with source/provenance metadata.
4. **Sacred Write Path enforced by seam-owned tests.** The three
   write-path tests pass in their owning crates: `seq_id` monotonic
   under concurrency in `bt-session` (3.4a); commit-before-broadcast
   ordering in `bt-runtime` (3.4b); store-failure halts turn through
   the HTTP boundary in `bt-server` (3.4c). After 4.2b's runtime
   relocation lands, 3.4b must still pass (and may need to follow the
   relocated commit emission path). This DoD bullet exists as a
   standalone gate so any future commit/broadcast drift fails CI
   loudly rather than silently undermining canonical inspection truth.
5. **Turn orchestration lives in `bt-runtime`.** `bt-server` no longer
   owns the turn-orchestration loop; the move from
   `bt-server::run_session_turn_with_turn_id` into
   `bt-runtime::TurnOrchestrator::run_session_turns` is complete (4.2b),
   landed only after 5.4a-min and 5.4a-control passed as behavioral
   gates, and the relocated path is covered by the same canonical event,
   approval, control, and inspection tests as the pre-relocation path.
6. **Bundle export fidelity.** Bundle export remains the full-fidelity
   raw-chunk artifact and round-trips a representative session without
   losing chunk lineage or related canonical event context. Backed by
   5.8's bundle round-trip assertion.
7. **OTLP export fidelity.** A real OTLP consumer (Phoenix or
   equivalent) ingests the OTLP bundle via the existing live test at
   `crates/bt-server/src/tests.rs`
   (`live_otlp_push_to_arize_preserves_session_correlation_across_turn_traces`),
   and the extended OpenInference
   token attributes from 3.3 are visible.
8. **JSONL and ShareGPT derived views.** JSONL and ShareGPT round-trip
   a representative session as truthful derived views over canonical
   session state. Backed by 5.8's JSONL and ShareGPT round-trip
   assertions. ShareGPT's loss surface (approval state, budget state,
   lineage) is pinned in the test file's comment block so future drift
   appears as a visible diff.
9. **HTML export renderability.** HTML renders without broken layout for
   large transcripts. Backed by 5.8's HTML renderability assertion
   against a 500-turn synthetic session.
10. **TUI is a usable primary interface (FR-16).** The TUI is the
    only human interface to Belltower during the foundation phase, so
    this bullet has four components that must all be green:
    - **Operator-surface readability.** Golden-output tests for
      `/help`, `/doctor`, `/status`, `/model`, `/inspect`, approval
      prompts, and error rendering pass against committed fixtures
      (4.3b).
    - **Long-session resilience.** The TUI handles a 500+ turn
      autonomous session without scrollback corruption or reconnect
      drops through the M4.1 stress invocation (5.6a).
    - **Interactive operator-scenario walk.** The scripted
      first-contact walk (login → doctor → model → first turn →
      approval → inspect → compact → export) runs cleanly through the
      opt-in full-screen tmux acceptance path (5.6b), while default
      `make acceptance` keeps the deterministic TUI invariant gate
      green.
    - **Dogfood friction surface documented.** The dogfood log
      includes the mandatory TUI friction section covering the six
      dimensions named in 5.7.
11. **Bounded working context.** No non-test source file exceeds 1,500
   lines without an explicit override entry (4.2). Every monolith
   named in the snapshot is either split below the cap or explicitly
   override-entered in `docs/development/file-size-overrides.md`.
12. **CI enforces invariants.** CI runs `fmt + check + clippy + test +
   purity + file-size + crate-boundary independence` on every push and
   passes (4.1).
13. **OpenAPI contract committed.** OpenAPI schema generated from
   `bt-server` routes + `bt-protocol` DTOs is committed and
   CI-enforced against drift (5.1).
14. **Reference client runs end-to-end.** `cargo run --example
   reference_client -p bt-client` runs end-to-end against a live
   `bt-server`, starting with `/server/info` capability negotiation
   (5.3).
15. **Scripted target milestones pass.** Milestones M0, M1, M2, M2.1,
   M3, M4, M5, M6, M7, M7.1, and M8 pass via `make acceptance` (4.4).
   `M4.1` passes via the dedicated long-session gate in 5.6.
16. **FR-9 instruction system is durably reconstructable.** Canonical
    instruction provenance is stored, and the reconstruction test from
    1.4 passes.
17. **Dogfooded.** The operator has used Belltower on Belltower for
    one full week of real work (5.7); friction items logged as
    issues. Dogfood week includes handing Belltower to at least one
    collaborator outside the core team via the installation path
    from Direction 7.
18. **Auth storage menu matches competitive bar.** All three storage
    modes (file, keychain, ephemeral) work end-to-end, with an
    `auto` mode selecting the best available backend at resolution
    time. `belltower doctor` reports which backend is active for
    each connection. Backed by 6.1, 6.2, 6.3.
19. **Installable from a source checkout or binary artifact.**
    The three `cargo install --path` commands succeed for Rust-toolchain users
    from a source checkout (7.1); platform binary release artifacts exist for
    macOS (arm64 + x86_64), Linux (x86_64 + aarch64), and Windows (x86_64) on
    release-tagged commits (7.2); and committed `docs/install.md` describes
    both supported install paths (7.3). Dogfood week confirms at least one
    collaborator reached `belltower doctor` via a documented path.

**Explicitly not in DoD:**

- Vertex / Gemini end-to-end readiness (Direction 2 expansion track)

## Out of Scope

Explicitly deferred. All valuable; none block the foundation. Landing
them on a solid foundation is strictly cheaper than landing them
alongside foundation fixes.

- **`bt-memory`** — cross-session semantic layer; build after
  dogfooding reveals what memory actually needs to remember.
- **Signals, reactions, playbooks, and hooks** — first-class ingress and
  lifecycle layers; build after the canonical operation/event and
  inspection contracts are stable enough for external producers and
  lifecycle callbacks to route through them.
- **`bt-dataset`** — export projection over the event log; build after
  the log has content worth exporting.
- **`bt-improve`** and Atropos training pipeline — requires
  token-level logprob capture and high-quality trajectories; both
  need foundation + dogfooding first.
- **`bt-research`** (autoresearch loop) — orchestration layer; build
  after delegation is proven.
- **Delegated / subagent execution** (Phase 4a/4b of the platform
  roadmap) — needs worktree policy and structured return artifact;
  follows dogfooding.
- **`bt-acp-adapter`** — build only when a concrete editor-integration
  customer exists.
- **Full provider-output / transcript-content algebra expansion** —
  reasoning, refusals, structured output, citations, media, and
  SnapshotPart / PatchPart-style content are significant schema
  changes; revisit after the foundation proves raw evidence,
  inspection, and current transcript/export paths under dogfood load.
- **Honcho provider, letta-style agent-identity memory, Langfuse
  observation bridge, tree-sitter repo map** — all downstream or
  optional.
- **Auth import flows.** Importing credentials from other installed
  tools (gcloud, Claude Code auth file, 1Password CLI, etc.) is
  deferred. Direction 6 establishes the storage-mode menu; import
  helpers are a separate capability for a later release.
- **Distribution beyond Homebrew / cargo / binary.** Package-manager
  coverage beyond what Direction 7 commits (apt, nix, winget, Chocolatey,
  Snap) is deferred. Foundation ships the three most universal paths;
  platform-specific packaging follows dogfood feedback.
- **IDE / editor integration.** No VS Code / Zed / JetBrains / Cursor
  integration in foundation. `bt-acp-adapter` above covers the
  editor-adapter story for post-foundation work.
- **Provider-native hosted search, browser automation, and broad web
  extraction.** Direction 8 pulls a small canonical `web_search` /
  `web_fetch` seam into foundation. Hosted provider search loops,
  browser automation, and a separate extraction subsystem remain
  deferred until dogfood shows they are worth the additional surface.

## Review Cadence

Update this document when:

- A task lands (mark it done inline with a commit SHA or PR link).
- A task reveals a hidden dependency (add it to Sequencing).
- Dogfood week surfaces a blocker that reshapes Out of Scope.
- The Current Code State Snapshot becomes stale (verify before any
  major task).

Do not update this document to describe work that has not happened.
When code and this plan disagree, code wins, and this document is
corrected.
