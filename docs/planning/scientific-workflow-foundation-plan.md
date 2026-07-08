# Scientific Workflow Foundation Plan

## Purpose

This plan defines the next foundation layer after
[`self-hosting-foundation-plan.md`](./self-hosting-foundation-plan.md).
The self-hosting foundation made Belltower usable as a standalone
multi-provider harness. This plan hardens the parts that matter when
Belltower becomes scientific infrastructure:

- what the model saw
- which generated context or compaction changed that view
- which tools were available, approved, executed, denied, or degraded
- how sessions can be searched, replayed, rewound, exported, and audited
- how future memory, signals, and context injection plug in without
  becoming hidden prompt text

The goal is not to add a large number of features. The goal is to make
the seams strong enough that future features do not create alternate
runtimes, alternate telemetry, or unverifiable prompt mutation.

This is an actively developing project. Backwards compatibility with
old local SQLite files or old DTO shapes is not a hard requirement when
a shape change improves the architecture. When docs, code, and this
plan disagree, apply the source-of-truth rules in
[`../development/source-of-truth-matrix.md`](../development/source-of-truth-matrix.md):
code and tests describe current behavior, architecture docs describe
long-lived invariants, and planning docs define the next intended work.
Do not preserve stale shapes for compatibility if they obscure the
canonical record, but do update the relevant architecture or subsystem
docs in the same slice so the new shape becomes explicit.

## Required Reading

Before implementing a slice from this plan, read:

1. [`../../AGENTS.md`](../../AGENTS.md)
2. [`../development/developer-guidelines.md`](../development/developer-guidelines.md)
3. [`../development/source-of-truth-matrix.md`](../development/source-of-truth-matrix.md)
4. [`../architecture/event-taxonomy-and-write-path.md`](../architecture/event-taxonomy-and-write-path.md)
5. [`../architecture/tool-system-and-normalization.md`](../architecture/tool-system-and-normalization.md)
6. [`../architecture/memory.md`](../architecture/memory.md)
7. [`../subsystems/session-runtime-and-agent.md`](../subsystems/session-runtime-and-agent.md)
8. [`../subsystems/tools-context-and-approvals.md`](../subsystems/tools-context-and-approvals.md)
9. [`../subsystems/telemetry-and-exports.md`](../subsystems/telemetry-and-exports.md)
10. [`../subsystems/core-protocol-and-server.md`](../subsystems/core-protocol-and-server.md)
11. The crate entrypoints and tests for the assigned slice.

## Guiding Principles

This plan applies the development principles in
[`developer-guidelines.md`](../development/developer-guidelines.md) to
scientific workflows:

- The harness is the product. The foundation work should strengthen
  orchestration, tools, telemetry, replay, context, and operator control
  rather than overfitting to one model or one benchmark.
- Complexity must pay rent. New shapes, projections, and protocols are
  justified only when they improve safety, observability,
  recoverability, extensibility, or operator leverage.
- Schemas and tools are infrastructure. Event shapes, DTOs, tool
  contracts, approval records, and context bundles are core design
  surfaces, not incidental implementation details.
- Context is a budget. Prefer explicit, durable, model-visible context
  attachments and compaction boundaries over hidden prompt mutation,
  stale memory, broad retrieval, or verbose instructions.
- Observability enables automation. Add durable telemetry, inspection,
  replay, regression checks, and cost accounting before adding more
  autonomous behavior.
- Recoverability beats brittle determinism. The system should make
  probabilistic failures explainable, interruptible, resumable,
  replayable, and auditable.
- Design for model evolution. Provider quirks and current model
  behavior should stay behind thin adapters and versioned public shapes.

## Current Snapshot

This plan is based on a read-only review of the current code and the
case-study harnesses under `docs/case-studies/harnesses/`.

Current Belltower strengths:

- `bt-core` already has typed events for instruction provenance,
  context manifests, compaction, tool operations, approvals, and raw
  completion chunks.
- `bt-runtime::TurnOrchestrator` records `turn.started`,
  `turn.instructions.recorded`, and `turn.context_manifest.recorded`
  before provider work.
- `bt-session` owns the canonical SQLite store and projections for
  messages, tool runs, approvals, cost, budget, queue/control/settings,
  and search over selected event kinds.
- `bt-otel` mirrors committed canonical events rather than owning its
  own source of truth.
- The TUI now behaves as the primary operator surface and consumes the
  protocol/client path rather than bypassing the runtime.

Current implemented foundation:

- Provider/auth/model preflight now gates model-visible context mutation
  on the runtime path, while launcher startup remains fast and degrades
  unreachable default providers truthfully.
- `ContextAttachment`, `ContextManifest`, compaction refs, instruction
  provenance, and projected turn-context inspection are implemented as
  canonical event/store surfaces.
- Compaction records stable `compaction_id` values and model-visible
  summary provenance that inspection/export can reference.
- Approval requests and resolutions are separate durable records with
  request-time evidence, fingerprints, policy metadata, and replayable
  decisions.
- Built-in tools, MCP tools, operator commands, `ask`, web search/fetch,
  and session search flow through the shared tool/operation telemetry
  discipline rather than TUI-local history.
- MCP inventory and degraded server states are inspectable without
  pretending unavailable tools were ready.
- Session search is exact, store-backed history search over canonical
  evidence; it is intentionally distinct from future semantic memory.
- `session.bt` portable bundles are manifest-owned artifacts with
  content-addressed raw chunks, artifact refs, offline validation,
  import/re-export fidelity, structural diff, continuation branches,
  event-boundary branch forks, and filesystem compare-and-swap push/pull.
- JSONL, HTML, ShareGPT, OTLP, and legacy debug bundle exports are
  derived consumers of canonical session state, not independent replay
  truth.

Remaining deliberate follow-on work:

- Full memory providers, semantic memory search, signal servers,
  playbooks, alternate protocol servers, and autonomous subagent
  spawning remain out of scope for this foundation layer.
- Long-running tool interruption has durable cancel/steer inspection and
  restart repair coverage, but individual executors can still add richer
  live process cancellation hooks as future hardening.
- Public registry transport, ORCID signing, publication curation, and
  remote multi-actor collaboration should build on `session.bt` and the
  CAS sync semantics instead of changing the local bundle contract.

## Case-Study Lessons

Use these as design input, not as code to copy blindly.

- **Codex:** strongest export/trace-bundle pattern. It separates
  append-only raw trace capture from offline interpretation. Belltower
  should copy the manifest-owned bundle idea, not make export depend on
  UI state or lossy projections.
- **pi-mono:** best compaction/session-entry pattern. Compaction records
  first-kept refs, summaries, branch state, and file provenance. This is
  close to the shape Belltower needs for durable context transitions.
- **Gemini / Qwen:** useful context-manager and memory-snapshot ideas,
  plus explicit stream-boundary tests. The risk is hidden prompt
  injection; Belltower should only adopt these patterns through
  canonical context attachments.
- **Hermes / Letta:** useful for memory-first and compressor-first
  workflows. Belltower should not copy opaque memory prompts; memory
  must be visible through manifest-backed attachments.
- **Goose / Opencode:** useful session search, rewind, ACP/MCP, and
  tool presentation patterns. Rewind should fork or mark projections,
  not mutate raw history.

## Non-Negotiable Invariants

- The canonical event store remains the source of truth.
- The sacred write path remains typed event -> SQLite commit ->
  post-commit fanout. UI, tracing, and export are consumers.
- `bt-agent` remains storage-agnostic and server-agnostic.
- `bt-runtime` owns turn execution, control transitions, and context
  preparation orchestration.
- `bt-context` owns context assembly/rendering algorithms and future
  context attachment composition.
- `bt-session` owns storage, projections, search indexes, and replayable
  inspection.
- `bt-protocol` owns public DTO shape changes.
- Every model-visible generated artifact must have durable provenance.
- Memory and signals must enter through explicit context attachments,
  not hidden prompt mutation.
- Built-in tools, MCP tools, operator shell, slash commands, `ask`, and
  future workflow operations must share the same operation/telemetry
  principles even when their execution backends differ.
- OTLP/OpenInference export is derived from canonical session data; it
  is not the replay source.
- Provider/auth/model usability is checked before any durable
  model-visible context mutation unless the failure itself is recorded
  as an explicit lifecycle event.
- Branch lineage is for speculative continuation inside one session.
  Child-session lineage is for delegated, spawned, or multi-agent work.
  Rewind should default to a child branch unless a separate delegation
  contract explicitly creates a child session.
- Session history search is not memory. It is an exact, store-backed
  inspection surface over canonical session events. Semantic memory and
  cross-session retrieval must enter later through distinct memory
  bindings and context attachments.

## Execution Model

Use small stacked branches. One writer owns each hotspot at a time.
Explorer/verifier agents may run in parallel, but writer lanes must not
overlap on these hotspots:

- `hotspot:bt-core-events-types`
- `hotspot:bt-context`
- `hotspot:bt-runtime-turn`
- `hotspot:bt-session-store`
- `hotspot:bt-protocol-dto`
- `hotspot:bt-server-routes`
- `hotspot:bt-tools-mcp-approvals`
- `hotspot:bt-otel-export`
- `hotspot:bt-tui-operator`

Each code slice should include:

- Problem: the observable gap being fixed
- Seam: the owning crate and consumer crates
- Shape: any event/DTO/projection/schema changes
- Behavior: the runtime/server/tool changes
- Tests: targeted crate tests plus any workspace/acceptance gate
- Docs: subsystem or architecture doc updates in the same branch

If implementation reveals that this plan is too specific or stale,
update the plan first. Prefer the invariants above over preserving a
premature task detail.

Keep each slice reviewable. A slice may change multiple crates when a
data shape crosses boundaries, but it should still have one primary
semantic owner and one acceptance story. If a planned slice starts to
become a broad refactor, split it at the shape boundary first.

## Phase 0: Snapshot And Test Baseline

Purpose: make the next work safe to parallelize.

Tasks:

- Refresh current-state claims in this plan before assigning writers.
- Add or update a lightweight tracker row in
  [`foundation-execution-tracker.md`](./foundation-execution-tracker.md)
  for any active writer branch.
- Identify old SQLite migration compatibility assumptions that can be
  dropped. Since this is active development, use explicit cutover notes
  where cleaner schemas are worth it.
- Align this plan with the current docs before code work starts:
  branch-vs-child-session semantics, export ownership, runtime
  preflight ordering, attachment ownership, and projection-backed
  inspection should be explicit.

Exit checks:

- `git status --short` is clean before each writer branch.
- Each writer branch has one reserved hotspot lane.
- The implementation slice cites the snapshot it verified.
- The plan and docs index are tracked before implementation branches
  start, so all agents work from the same source of truth.

## Phase 0.5: Runtime Preflight And Context Mutation Boundary

Purpose: prevent durable context facts from being written for work that
could not actually execute under the selected provider/auth/model path.

Seam:

- Owner: `bt-runtime`, `bt-providers`, `bt-auth`, `bt-readiness`
- Consumers: `bt-server`, `bt-protocol`, `belltower`, `bt-tui`

Design direction:

- Share one credential/model compatibility decision between readiness
  and runtime provider construction. Readiness can remain fast and
  bounded at startup, but runtime execution must not accept a credential
  kind or model shape that the configured provider cannot use.
- Validate the executable provider/auth path before recording
  model-visible context mutations such as compaction, or introduce an
  explicit failed-preflight context lifecycle event with clear status and
  no ambiguous model-visible effect.
- Server adapters and approval/resume paths must enter the same runtime
  compatibility gate. Do not let direct server continuation or
  retry/resume code construct a provider path that readiness/runtime
  preflight would reject.
- Keep launcher startup usable: slow network readiness should degrade
  asynchronously where possible, but it should not block local startup
  or produce contradictory `/use` status.
- Document the intended fast-start behavior if the implementation
  intentionally waits for TCP health first and refreshes richer
  readiness after launch.

Tests:

- Provider-construction and auth-preflight failures, including missing
  credentials, expired credentials before refresh, unsupported
  credential kinds, and unavailable configured models, do not create a
  successful context/compaction record for a turn that never reaches an
  executable provider call.
- Provider-call failures after executable preflight may have a context
  manifest, but they must record explicit failed completion/turn
  lifecycle state and must not imply that model output exists.
- Readiness and runtime provider construction agree on unsupported
  credential/model combinations.
- Startup with an unreachable default cloud provider falls back to a
  usable local connection quickly and records the degraded reason
  truthfully.

Docs:

- Update `session-runtime-and-agent.md`,
  `providers-auth-and-models.md`, and
  `core-protocol-and-server.md`.

## Phase 1: Canonical Model-Visible Turn Input

Purpose: make “what the model saw” a durable, projection-backed,
exportable fact.

Seam:

- Owner: `bt-core`, `bt-context`, `bt-runtime`, `bt-session`, `bt-protocol`
- Consumers: `bt-server`, `bt-protocol`, `bt-otel`, `bt-tui`

Design direction:

- Add a typed `PreparedTurnInput` or `TurnContextBundle` shape. It
  should reference:
  - turn id, branch id, settings revision, provider, model
  - rendered system prompt provenance
  - message refs with event seq ids and branch bounds
  - tool inventory refs
  - compaction refs
  - context attachments
  - planned provider-call id or LLM-call ordinal boundaries; raw chunk
    ranges are linked later by projections/export after chunks exist
- Add `ContextAttachment` shapes for future memory/signals without
  implementing full memory yet. These shapes should stay generic; they
  should not bake in one memory provider, one signal server, or one
  scientific workflow. Minimum fields:
  - attachment id and type (`generated`, `compaction`, `memory`,
    `signal`, `resource`)
  - producer (`runtime`, `operator`, `tool`, `memory_provider`,
    `external_server`)
  - source refs and hashes
  - visibility (`model_visible`, `operator_visible`, `exportable`)
  - placement in the prompt/request
  - token estimate and truncation status
- Make `turn.context_manifest.recorded` point at the prepared input or
  carry enough refs to reconstruct it.
- Add `bt-session` projections for turn context, instruction
  provenance, context manifests, compactions, and attachment refs.
- Expose the prepared input through protocol inspection before the TUI
  renders it. `bt-tui` and future clients should consume the DTO rather
  than rescanning raw events or reconstructing prompt state locally.
- Keep `bt-context` responsible for assembly/rendering and `bt-session`
  responsible for projection-backed reads. Do not move prompt assembly
  into storage, and do not make runtime inspection scan serialized event
  blobs for normal operator reads once the projection exists.

Tests:

- A turn with user input, system instructions, tools, and context
  manifest can be reconstructed from projections without rescanning all
  serialized event blobs.
- A generated context attachment appears in inspection/export and is
  visible in the model request exactly once.
- A branch replay can identify the exact context boundary for a turn.

Docs:

- Update `session-runtime-and-agent.md`,
  `telemetry-and-exports.md`, and `memory.md`.

## Phase 2: Compaction As Durable Context Transition

Purpose: make compaction correct, replayable, and scientifically
auditable.

Seam:

- Owner: `bt-context` for compaction algorithms, `bt-runtime` for
  invocation, `bt-session` for durability/projections
- Consumers: `bt-otel`, `bt-tui`, export/search

Design direction:

- Give every compaction a stable `compaction_id`.
- Link the synthetic summary message or generated attachment directly
  to the `context.compacted` event.
- Record trigger, reason, phase, status, message/seq range, branch id,
  first-kept ref, token before/after, files read/modified, provider,
  model, latency, and failure state.
- Make `compaction_threshold` and `summary_connection` either active
  behavior or explicitly documented as future config.
- Preserve tool-call/result adjacency and file provenance across
  compaction.
- Decide whether failed preflight compactions can exist without
  `turn.started`; if yes, make that lifecycle explicit in events and
  inspection.
- Treat branch handoff summaries and compaction summaries as
  model-visible generated artifacts when they are injected into a
  request. They need attachment/message refs just like other generated
  context, not anonymous synthetic strings.
- Do not let compaction become a hidden summarization layer in
  `bt-agent`. The agent receives already prepared context; runtime and
  context own the compaction decision and durable transition.

Tests:

- Long session compacts and later reconstructs the model-visible
  request with the compaction summary linked to its producer event.
- Tool calls and results are never separated by compaction.
- Repeated compactions preserve branch lineage and file provenance.
- Failed/inflated compaction records an explicit failed context
  transition and does not silently mutate model input.

Docs:

- Update `session-runtime-and-agent.md`,
  `provider-output-and-transcript-model.md`, and
  `telemetry-and-exports.md`.

## Phase 3: Unified Tool, Approval, And MCP Operation Contract

Purpose: make every meaningful operation inspectable across human,
agent, built-in, MCP, and future workflow surfaces.

Seam:

- Owner: `bt-core`, `bt-protocol`, `bt-tools`, `bt-mcp`, `bt-runtime`, `bt-session`
- Consumers: `bt-server`, `bt-protocol`, `bt-tui`, `bt-otel`

Design direction:

- Expand approval request durability. Store the approval request facts,
  not only pending status:
  - arguments hash and optional redacted argument preview
  - tool metadata and risk class at request time
  - approval requirement and policy rule
  - initiator and surface
  - registry/tool schema hash
  - durable request fingerprint
- Treat the approval request snapshot as the durable object that later
  approval resolution references. Resolution should not depend on the
  current registry, current MCP server state, or current policy wording
  to explain an old request.
- Keep `ApprovalRequestSnapshot` and `ApprovalResolution` as separate
  canonical records and DTOs. The snapshot is immutable request evidence;
  the resolution records decision kind, scope, decided_by, reason, and
  request fingerprint.
- Name the producer seam for approval request facts. Either `bt-agent`
  carries richer pending-approval data to runtime, or `bt-runtime`
  constructs the immutable request snapshot before suspension. Do not
  leave approval replay dependent on the current tool registry.
- Populate `ToolOperationContext` consistently for:
  - agent tool calls
  - operator shell
  - slash commands that affect session state
  - `ask`/pending input
  - MCP tools
  - future workflow operations
- Persist MCP inventory snapshots used for each turn:
  - configured/discovered/ready/degraded server states
  - skipped tools and reasons
  - tool schema hashes and policy classifications
  - trust/include/exclude decisions
- Keep historical placeholder metadata for pending/replay if an MCP
  server disappears.
- Reserve fields for active execution state and interruption outcomes
  while designing operation snapshots. Phase 4 will use the same seam
  for cancel/interrupt behavior; avoid adding a parallel cancellation
  vocabulary later.
- Record validation-preflight outcomes as operation/policy evidence when
  they block or permit mutation. The existing exact-output preflight is
  a harness policy decision, not a TUI-local or server-local warning.

Tests:

- An approval requested before a tool-policy change remains auditable
  and resolvable from stored request facts.
- Operator shell and agent shell both emit durable operation context
  with initiator, approval/policy state, and artifact refs.
- MCP degraded/skipped tools appear in turn inspection and export
  without pretending they were available.
- Built-in and MCP tools satisfy the same telemetry/export invariants.

Docs:

- Update `tools-context-and-approvals.md`, `mcp.md`,
  `tool-policy.md`, and protocol docs if DTOs change.

## Phase 4: Runtime Controls For Long Scientific Work

Purpose: make cancel, steer, pause/resume, restart repair, and
long-running tool interruption dependable.

Seam:

- Owner: `bt-core`, `bt-runtime`, `bt-session`
- Participants: `bt-agent`, `bt-tools`, `bt-mcp`
- Consumers: `bt-server`, `bt-tui`, `bt-protocol`

Design direction:

- Add an explicit active-turn/tool lease or heartbeat if needed for
  restart repair.
- Repair interrupted active work on restart into a canonical terminal
  state rather than leaving ambiguous live state.
- Make cancel/steer semantics branch-scoped or turn-scoped explicitly;
  avoid session-global ambiguity.
- Add active tool cancellation hooks for shell, MCP, web fetch, and other
  long-running operations.
- Carry cancellation capability through the typed tool execution seam
  and the `bt-agent` loop. Long-running tool interruption is not only a
  runtime/session concern; the executor boundary must be able to observe
  and report interruption without inventing tool-specific side channels.
- Record cancellation outcome on the affected tool run and turn.
- Keep server handlers as thin transport into runtime transitions.
- Build on the Phase 3 operation context rather than inventing a second
  live-execution status model. If a field is replayable, it belongs in
  canonical session state; if it is live-only, name it as live-only in
  DTOs so clients do not mistake it for durable truth.

Tests:

- Start a long-running tool, request cancel, restart, and inspect a
  truthful terminal state.
- Steer/cancel does not affect the wrong branch.
- Queued input and pending ask survive restart and inspect correctly.
- Resume emits durable turn boundaries before resumed provider/tool
  events.

Docs:

- Update `session-runtime-and-agent.md`,
  `core-protocol-and-server.md`, and `tui-architecture.md`.

## Phase 5: Manifest Export, Raw Integrity, And Session Search

Purpose: make Belltower session data usable as a standalone scientific
artifact.

Seam:

- Portable bundle owner: `bt-core`, `bt-session`
- File/archive protocol owner, when exposed over HTTP: `bt-protocol`
- Derived export consumers: `bt-server`, `bt-client`, `bt-tui`, `bt-otel`,
  external tools

Design direction:

- Treat the existing `/export legacy-bundle` JSON response as a stale transitional
  export/inspection surface. This is an active-development cutover, so do not
  preserve the legacy JSON shape for compatibility if it conflicts with the
  portable session model. The portable scientific artifact is a manifest-owned
  `session.bt` bundle, not a silent expansion of that JSON response:
  - `manifest.json`
  - append-only raw event JSONL
  - `raw_chunk_refs.jsonl`
  - raw provider chunks
  - payload/artifact directory
  - schema versions and producer versions
  - privacy/redaction flags
  - manifest-owned `SessionRecord`, branch ids, and external node refs
  - stable node refs for cross-bundle continuation
  - content refs for raw chunks, payloads, and artifacts
- The first implementation slice is intentionally local and verifier-first:
  `belltower session export <session-id> --out <dir>` writes an unpacked
  `session.bt` directory, and `belltower session validate <bundle>` verifies it
  offline. Local `belltower session import <bundle>` rehydrates the same
  canonical evidence into SQLite and proves re-export fidelity. Local
  `belltower session diff <a> <b>` compares portable event hashes, branch
  inventory, content inventory, and redaction state so bundle review does not
  depend on local SQLite ids. Local `belltower session continue <bundle>`
  creates a continuation branch from a selected portable node ref and records
  the external parent ref in the canonical branch event. Private
  filesystem-backed `belltower session push <bundle> --remote <dir>` and
  `belltower session pull --remote <dir> --session <id> --out <dir>` exercise
  compare-and-swap branch-head updates before any public HTTP archive transport
  lands.
- Artifact modes are part of the bundle contract, not a separate publication
  system. `trace-only` exports only canonical evidence, `trace-plus-artifacts`
  includes operator-selected files under the session project root, and
  `trace-plus-patches` can include an explicit or generated Git patch artifact.
  Belltower should not infer publishable artifacts by scanning the project
  workspace.
- Define portable identity before behavior:
  - `event_id` is preserved across export/import
  - local SQLite `seq_id` is debug/display metadata and may change on
    import
  - portable hashes use the concrete `sha256:<lowercase-hex>`
    representation in the first implementation
  - `event_hash` is computed from documented canonical event bytes
  - bundle event ranges use bundle-local ordinals plus hash continuity,
    not imported-store sequence ids
  - `bundle_hash` is the hash of `checksums.json`, whose entries cover
    non-manifest bundle contents. `manifest.json` carries that bundle hash and
    is validated for consistency without being a checksum entry itself.
- Split protocol surfaces before implementation:
  - projection exports such as JSONL, HTML, ShareGPT, and OTLP may keep
    using `SessionExportResponse { content: String }`
  - portable `session.bt` export/import/validate/diff use a distinct
    file/archive-aware session-bundle surface
  - the old `ExportFormat::Bundle` path is renamed to
    `ExportFormat::LegacyBundle` and remains legacy/debug JSON rather than
    becoming the new bundle API
- Treat transcript, OTLP/OpenInference, ShareGPT, HTML, search indexes,
  and other projections as derived outputs. They may be regenerated at
  runtime or emitted as optional caches, but they are not required for
  validating or importing a `session.bt` bundle.
- Add an offline verifier before adding new export formats:
  - contiguous bundle event ordinal ranges
  - raw chunk hash and byte counts
  - payload exists before event refs
  - no dangling parent/branch refs
  - paired provider stream boundaries
  - deterministic reducer output
- Add a first-class `belltower session validate <bundle>` command before
  remote sync so external readers can verify a bundle offline.
- Add a first-class `belltower session diff <a> <b>` command for
  same-lineage updates, fork divergence, replication comparisons, and
  redaction previews. The first slice is structural and local: it classifies
  equivalent bundles, same-lineage updates, fork divergence, different lineage,
  redaction changes, and non-event structural differences.
- Use the target primitives from
  `docs/architecture/session-bundles-and-trace-sync.md`:
  `SessionNodeRef`, `ContentRef`, `EventRange`, `BundleArtifactRef`,
  and the bundle manifest. Do not collapse `BundleArtifactRef` into the
  existing tool-operation `ArtifactRef` without an explicit data-shape
  decision.
- The bundle manifest owns the full `SessionRecord`, not a bare top-level
  `session_id`, because import must recreate project root, connection/model,
  settings revision, tool mode, and parent refs without heuristic defaults.
- Include deterministic raw chunk content refs in the first schema/export
  slice. Do not defer raw chunk identity to a later cleanup, because bundle
  validation, import, re-export, and diff all depend on it.
- Remote push/pull should operate on immutable event ranges and
  content-addressed payloads. Branch-head updates use compare-and-swap:
  if the expected remote head does not match, the push rejects and the
  client must pull and inspect or diff before retrying or creating a
  continuation branch.
- Keep OTLP/OpenInference as a derived export with links back to canonical
  event ranges and raw content refs.
- Export, validate, import, diff, push, pull, share, and publish
  commands must record compact operation summaries when they affect
  operator-visible truth or durable workflow state. Summaries include
  manifest hash, verification status, redaction policy, output location
  or remote ref, and sync head movement when applicable. They should not
  record the exported content preview back into the same session in a
  way that changes the evidence boundary being exported.
- Split search into two explicit surfaces. In-session evidence search is
  projection-backed search over canonical events and raw evidence. Cross-
  session discovery searches session metadata and summaries with a
  stable cursor.
- In-session evidence search should support:
  - title/first prompt/summary
  - message text
  - tool names and file paths
  - model/provider/cwd/branch
  - context attachment refs
  - raw evidence anchors
- Cross-session discovery should support title, first prompt, summary,
  model/provider/cwd, updated time, and cursoring by `(updated_at, id)`
  or a similarly stable cursor.
- Keep JSONL, HTML, ShareGPT, and OTLP exports as consumers of the same
  manifest/reducer model where possible. Do not let any export format
  grow its own reconstruction rules that disagree with the canonical
  bundle verifier.

Tests:

- Existing tests that assert the old portable bundle shape should be rewritten
  or renamed as legacy/debug JSON tests. New portable-bundle tests should not
  require `messages`, `message_branch_id`, or local raw chunk row ids inside the
  canonical bundle.
- `session.bt` export validates offline after process restart.
- `belltower session validate <bundle>` rejects malformed manifests,
  bad hashes, missing payloads, non-contiguous ranges, and dangling refs.
- Import/re-export preserves `event_id`, `event_hash`, and external
  `SessionNodeRef`s even when local `seq_id` values differ.
- `belltower session diff <a> <b>` identifies same-lineage updates,
  fork divergence, and redaction effects structurally.
- Reducer reconstructs transcript, model-visible inputs, tool calls,
  compactions, and raw provider refs.
- `belltower session continue <bundle> [--event-hash sha256:...]`
  reconstructs context only up to that node and records the external
  parent node ref.
- `belltower session push <bundle> --remote <dir>` rejects stale expected
  branch heads, and `belltower session pull --remote <dir> --session <id>
  --out <dir>` validates the pulled bundle without mutating SQLite.
- `belltower session export <session-id> --out <dir> --artifact <path>`
  writes a trace-plus-artifacts bundle with artifact refs tied to a
  `SessionNodeRef`, and validation rejects missing or root-escaping artifact
  content.
- Search returns stable paged results and does not miss same-time
  sessions.
- OTLP spans include session/turn/event-seq correlation and do not
  become replay truth.

Docs:

- Update `telemetry-and-exports.md`, `core-protocol-and-server.md`,
  and API docs if protocol surfaces change.

## Phase 6: Rewind, Replay, And Branch Semantics

Purpose: support scientific exploration without mutating evidence.

Seam:

- Owner: `bt-session`, `bt-runtime`, `bt-protocol`
- Consumers: `bt-server`, `bt-client`, `bt-tui`, export/search

Design direction:

- Implement branch-from-turn or branch-from-event-seq as a forked branch
  with an explicit parent branch boundary. The local protocol path now
  supports event-boundary forks via `CreateBranchRequest.from_event_id`;
  follow-on UX can expose turn/node selectors on top of that canonical seam. A
  child session is only appropriate when the operation is delegated/spawned work
  and the session-graph contract says so.
- Do not mutate raw history for rewind.
- Represent rewind/fork as canonical branch or rewind operation events,
  plus an optional file restore operation with explicit artifact refs.
  Projections derive branch state after those events commit; projections
  are not the write target.
- Bound replay by event seq, branch, settings revision, context bundle,
  and raw provider refs.
- Keep file-system restore separate from conversation rewind unless a
  file snapshot/diff artifact exists.
- Make replay boundaries name the context bundle, settings revision,
  branch boundary, and raw provider refs used for the resumed or forked
  work. Replaying transcript text alone is not sufficient.

Tests:

- Branch from a past turn and continue without altering parent raw
  events.
- Export shows parent/forked branch lineage and branch boundary.
- Search can include or exclude child branches/sessions.
- Optional file restore is recorded as an operation with artifact refs.

Docs:

- Update `overview.md`, `session-runtime-and-agent.md`, and
  `telemetry-and-exports.md`.

## Phase 7: Operator Surfaces

Purpose: expose the foundation without making the TUI a second source
of truth.

Seam:

- Truth producers: `bt-session`, `bt-runtime`, `bt-server`
- Wire and presentation owners: `bt-protocol`, `bt-client`, `bt-tui`, `belltower`
- Consumers: human operators and external clients

Design direction:

- Add TUI/API inspection for:
  - model-visible turn input
  - context attachments
  - compactions
  - approval request snapshots
  - MCP inventory snapshots
  - raw/export verification status
  - search and branch/replay boundaries
- Keep the TUI display-only over canonical state. No TUI-local
  transcript/control truth.
- Make slash/operator commands durable named operations when they
  affect behavior or operator-visible truth.
- Render export/search/replay results from canonical DTOs and record
  operation summaries rather than local UI buffers or raw export
  previews.
- Expose session search in the operator surface as exact canonical
  history search, with clear references back to event seq, turn, branch,
  tool call, and context attachment where present.
- Keep local defaults and launcher bootstrap state distinct from
  canonical session truth. Operator convenience state should not become
  hidden runtime state.

Tests:

- TUI can inspect a turn and show context/compaction/tool facts from
  canonical DTOs.
- Slash command output is recorded as durable operator history.
- Export/search/replay commands do not reconstruct state from local UI
  buffers.

Docs:

- Update `launcher-and-tui.md`, `tui-architecture.md`, and
  `tui-cutover-spec.md`.

## Phase 8: Scientific Workflow Acceptance Harness

Purpose: prove the foundation with a real workflow before adding larger
features.

Representative scenario:

1. Start Belltower with a local or cloud provider.
2. Run a multi-turn task that reads and writes project files.
3. Trigger at least one approval, one `ask`, one compaction, one MCP or
   degraded-MCP inventory event, one operator command, and one
   interruption or resume path.
4. Export the session bundle.
5. Verify the bundle offline.
6. Replay or branch from a previous turn.
7. Search for a tool/file/context artifact and inspect its raw evidence.
8. Emit OTLP/OpenInference and verify span-to-event correlation.
9. Verify that exact-output or file-producing work records the
   validation surfaces the agent inspected before mutation.

Acceptance:

- The workflow can be understood six months later from canonical
  session data without access to the original running process.
- The TUI, API, portable bundle, legacy-bundle debug export, and OTLP view tell
  the same story with
  explicit fidelity boundaries.
- Any missing data is classified as a known limitation, not silently
  inferred.

Implemented deterministic closeout:

- `make acceptance` is the default foundation gate. It runs deterministic
  server/API, runtime, session, tool, MCP, TUI, protocol, export, and
  portable `session.bt` checks without live provider credentials.
- The representative inspection-contract test covers a session with a
  durable operator command, built-in approval, MCP approval, raw chunks,
  canonical queue/execution/tool inspection, MCP inventory, and legacy
  debug export.
- Portable bundle tests cover offline validation, import/re-export
  fidelity, structural diff, continuation from a selected node,
  event-boundary branch forks, selected artifacts, raw-chunk integrity,
  and filesystem CAS push/pull.
- Live provider, long-running tmux TUI, and dogfood-week gates remain
  explicit operational checks because they depend on local credentials,
  terminal state, or real development work rather than deterministic
  unit fixtures.

## Out Of Scope

These remain future work unless this plan is updated first:

- full memory provider implementation
- semantic memory search
- signal/reaction/playbook execution
- hooks beyond existing tool/runtime surfaces
- dataset/training artifact productization
- autonomous subagent spawning inside Belltower
- ACP or alternate protocol servers
- web/IDE client implementation
- new benchmark suites beyond the lightweight regression/eval harness
  needed to validate the foundation

The plan intentionally adds attachment and manifest seams so those
systems can arrive later without violating the event-store-first
architecture.

## Implementation Status

The foundation slices in this plan have landed on `main`. The remaining
work items are intentionally outside this foundation closeout unless a
future plan promotes them:

- full memory and signal-provider implementations
- public registry transport and signed publication workflows
- richer executor-specific live cancellation hooks
- autonomous subagent execution inside Belltower
- semantic or cross-session memory retrieval

Future work should keep using the same seams: typed events first,
projection-backed inspection, canonical `session.bt` bundle semantics,
and TUI/API surfaces as consumers rather than truth producers.
