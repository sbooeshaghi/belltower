# Telemetry and Exports

This document covers:

- the Belltower-owned event model in `bt-core` with OI-compatible projection
- the canonical session store in `bt-session`
- runtime emission behavior in `bt-runtime`
- export surfaces in `bt-otel`

This is the main differentiator of Belltower.

Read it with:

- [`../architecture/event-taxonomy-and-write-path.md`](../architecture/event-taxonomy-and-write-path.md)

## Purpose

Belltower is being built so the harness itself owns the telemetry.

That means:

- the session is the canonical telemetry record
- raw provider behavior is durable
- OpenInference alignment exists at the event model level
- OpenTelemetry export is derived from stored session data
- the typed event log is primary; `tracing`/OTel is a mirror, not the source of truth

This is not "observability added later."
It is part of the product boundary.

## Canonical Telemetry Record

The canonical record lives in Belltower's session/event store.

That record must be rich enough to reconstruct:

- assistant and tool behavior
- token and cost accounting
- branch history
- approval decisions
- model-visible context manifests for each provider call
- tool-operation metadata and artifact references
- provider raw stream behavior
- parent-child session lineage and typed related-session mailbox delivery,
  claim, and resolution state

The intended runtime wiring is:

1. construct a typed `EventEnvelope`
2. commit it to SQLite
3. fan out the committed event to post-commit projections:
   - SSE/UI consumers
   - the `tracing` mirror
   - export/index refresh work
4. export OTLP/OpenInference views from the canonical session data and mirrored runtime traces

This gives one canonical event with multiple projections, instead of making `tracing` the primary persistence layer.

The runtime tracing bridge should be post-commit and loss-tolerant:

- the mirror happens only after SQLite commit succeeds and `seq_id` is assigned
- the mirror does not mutate or enrich the canonical event in place
- large fields are truncated in the tracing projection; the full payload remains in SQLite

The live UI stream must follow the same durability boundary:

- SSE broadcast is driven by the committed envelope, not by the tracing mirror
- a dropped tracing event is an observability problem
- a dropped canonical SSE event is a product-correctness problem

The two post-commit projections should be independent even when they are triggered from the same write path.

The runtime should also maintain derived execution structure around live work.

The important external projection rule is:

- a Belltower session is the long-lived correlation container
- a turn is the exported OpenInference trace boundary
- `llm_call` and `tool_execution` are spans inside that turn trace

These traces and spans are derived views over the canonical session log. They help OTEL consumers render timing, nesting, span kinds, and high-signal inputs and outputs, but they do not replace the canonical `EventEnvelope` stream.

The important modeling rule is:

- `turn` is the human/harness work unit
- `llm_call` is one actual provider request/response round-trip inside that turn
- `tool_execution` spans are siblings of `llm_call` spans under the same `turn`
- provider chunks remain events on the active `llm_call`, not their own spans
- `turn.context_manifest.recorded` is the durable pre-call record of which
  branch, messages, tools, settings revision, attachments, and prompt surface
  were visible to that `llm_call`
- successful compaction is not just a transcript rewrite: `context.compacted`
  carries the stable compaction id, trigger, phase, status, provider/model,
  boundary sequence, summary-message reference, first-kept message/source
  reference when available, latency, and before/after accounting
- context manifests attach the same compaction id as a model-visible/exportable
  `compaction` attachment, giving telemetry and exports a direct join between
  the event that changed context and the provider call that consumed it
- `tool.operation.recorded` is the durable policy/operation envelope for tool-like work; arguments and results remain in `tool.call.requested` and `tool.execution.finished`

The only pre- or non-commit work allowed in `tracing` is this instrumentation layer.
If a user message, assistant result, approval, tool call, or canonical error shows up in `tracing` without a prior canonical commit, that should be treated as an architectural bug.

## OpenInference Alignment

The event model should support OI-compatible attributes for:

- sessions, turn-derived traces, and spans
- turns
- agent spans
- LLM spans
- tool spans
- chain-level spans
- provider/model metadata
- token usage
- cost metadata
- structured inputs and outputs
- OpenInference span kinds and flattened message/tool attributes that consumers such as Phoenix use for rich rendering

This belongs in core types and stored events, not in a separate export-only adapter.

The alignment is structural, not nominal.
Belltower should keep stable internal field names such as:

- `provider`
- `model`
- `input_tokens`
- `output_tokens`
- `tool_name`

and let `bt-otel` map those to the current OI semantic conventions.
The core event model may carry attribute-hint keys where useful, but the canonical schema should not be forced to rename itself every time the OI spec evolves.

Turn lifecycle should be explicit and queryable:

- `turn.started`
- `turn.finished`

Every event that is part of a runtime turn should carry a turn identifier so the stored session log can be projected back into reproducible turn boundaries.

The intended relationship between spans and canonical events is:

- the server owns `session` and `turn` spans
- the agent loop owns `llm_call` and in-loop `tool_execution` spans
- approval-resume tool execution uses the same `tool_execution` span shape from the server path
- committed canonical events are mirrored into whatever span context is currently active

That means tool-using turns may contain multiple `llm_call` spans:

- initial model round-trip
- zero or more tool executions
- follow-up model round-trips after tool results or resumed input

The current `bt-otel` export now maps the canonical store into richer OpenInference-style spans:

- each session export may contain many OTLP traces, one per turn in the session
- each exported trace uses the turn as the root `AGENT` span
- every exported span carries `session.id = <belltower session_id>` for cross-trace correlation
- `llm_call` exports as an `LLM` span with:
  - `input.value` / `output.value`
  - flattened `llm.input_messages.*`
  - flattened `llm.output_messages.*`
  - token and cost attributes
  - canonical context-manifest metadata for visible branch/message refs, tools,
    settings revision, attachment refs, boundary sequence, and compaction status
- `tool_execution` exports as a `TOOL` span with:
  - tool identity
  - JSON input/output payloads
  - approval and error metadata
  - operation metadata and artifact refs when present in canonical events
- OTLP span events include payload previews and mirrored event attributes so the event stream is useful inside Phoenix instead of only showing bare event names
- related-session mailbox events mirror message direction, peer session,
  delivery mode, kind, status, and message/turn correlation; the canonical
  paired session events remain the source of truth

Portable session bundles preserve each session's own mailbox event copy. When
related parent and child bundles are imported into the same store, projections
reconstruct sent/received state from those canonical events; exporters do not
flatten child history into the parent bundle or invent a workflow transcript.

## Cost Accounting

Cost accounting should be treated as a canonical projection over:

- provider-reported or derived token usage
- the Belltower pricing catalog in effect when the event is committed

The important rule is that historical session cost surfaces should be stable.
Later pricing-catalog changes should not silently rewrite old session cost totals in place.
When a completion has no usable price, Belltower keeps
`total_cost_usd` unchanged and increments the session's unpriced-completion
counter so "free" and "not priced yet" remain distinguishable.

Session inspection exposes the current cost summary, including token totals,
priced dollar totals, and the unpriced-completion count. Runtime cost
calculation includes cache-read, cache-write, and reasoning-token classes when
the pricing catalog has rates for the reported class. If a provider reports a
non-zero token class whose public rate is not configured, the completion stays
unpriced instead of producing a misleading partial cost.

`bt-session` owns a reusable event-log reprojection seam for repairing derived
projection tables from stored canonical events. Cost reprojection rebuilds the
cost summary from stored `completion.finished` events with caller-supplied
pricing logic; it does not mutate the append-only event log or create synthetic
repair events.

## Session History Search

Session history search is an inspection/read surface over the canonical session
event log. It is useful for operator review and for agent self-inspection during
long sessions, but it does not introduce a second telemetry model.

The important constraints are:

- results come from stored session events, not from TUI-local scrollback or
  runtime-only state
- matches include canonical references such as sequence id, event id, turn id,
  branch id, and tool call id where available
- current-branch search follows branch lineage; all-branch search is explicit
- search does not mutate context, inject hidden memory, or alter replay
  semantics

## Raw Stream Durability

One of the most important design commitments is persisting both:

- structured event projections
- raw provider chunks

This allows:

- replay
- parser improvements without rerunning sessions
- debugging provider edge cases
- high-fidelity export and analysis

Raw chunk durability also needs query ergonomics.
The primary access patterns should include:

- all raw chunks for one turn on one branch
- all raw chunks for the Nth LLM call in a session
- comparing the finalized structured projection against raw provider chunks for the same turn

Current implemented state:

- raw chunks persist `session_id`, `branch_id`, `turn_id`, and `llm_call_ordinal`
- turn-scoped raw paging exists through the server/client/TUI path
- raw chunks now persist the canonical `event_id` of their paired `raw_chunk.persisted` event
- context manifests persist the same `turn_id` and `llm_call_ordinal`, plus the
  context boundary sequence and message source refs, so consumers can compare
  model-visible context, raw chunks, and finalized completion events without
  reconstructing the provider call from mutable current state
- compaction manifests include model-visible compaction attachments that point
  back to the durable compaction id and synthetic summary message, so long
  sessions can be inspected without guessing where summarized context entered
  the model call
- `SessionExecutionInspection` exposes those context manifests directly, and the
  TUI `/execution` view renders a compact context summary from that canonical
  inspection shape
- `/raw` renders raw chunks for a selected turn and can scope to one LLM call
- `/raw diff` now filters to the exact correlatable structured events for the selected raw chunk set rather than all events in the turn

That depends on durable identifiers and indexes that connect raw chunks back to:

- `session_id`
- `branch_id`
- `turn_id`
- provider/model or LLM-call ordinal
- chunk order

Without that, raw chunk durability becomes archival rather than operational.

## Concurrency And Failure Semantics

SQLite WAL mode gives Belltower one canonical writer per database and many readers.
When multiple sessions are active concurrently:

- writes still serialize through the same canonical writer path
- `seq_id` ordering remains global
- events from different sessions may interleave in the WAL and global sequence space

That is acceptable as long as session, branch, trace, and turn identifiers remain explicit.

If the canonical store is unavailable because of disk, lock, or migration failure, Belltower should fail closed for canonical session progression.
It should not silently fall back to a hidden non-durable mode for ordinary interactive work.

## Export Surface

Standalone completeness should include:

- JSONL export
- HTML export
- ShareGPT-style export
- OTLP/OpenInference export via `bt-otel`

Current implemented state:

- legacy JSON bundle export now exists as `/export legacy-bundle <path>`, carrying the session record, branch records, canonical events, default-branch message projection, and raw chunks in one typed JSON artifact
- JSONL, HTML, and ShareGPT exports exist through `/export <format> <path>`; running `/export` or `/export <format>` pre-fills the command through the format and leaves the cursor at the path position so the operator explicitly chooses the destination
- OTLP export now exists in two forms through `bt-otel`:
  - `/export otlp <path>` writes a canonical-store-derived OTLP/OpenInference JSON projection for human inspection
  - `/export otlp push [collector-endpoint] [project-name]` uploads real OTLP protobuf to a collector-compatible endpoint such as Phoenix
- OTLP export emits one trace per turn in the session:
  - `turn`
  - `llm_call`
  - `tool_execution`
- turn OTLP spans now carry canonical turn provenance including `settings_revision_id`, start source, and resumed call linkage when the turn was bootstrapped from approval or input resume
- related-session mailbox events carry explicit `belltower.related_message.*`
  attributes for message id, direction, peer session, delivery mode, message
  kind, resolution status, and resulting turn id instead of requiring consumers
  to parse a display summary
- canonical instruction provenance is stored as `turn.instructions.recorded`; exports consume that event as part of the session record instead of reconstructing prompts from current files
- approval-resume and input-resume tool spans are reconstructed from canonical events and reuse the original tool-call input when the execution lands in a later resumed turn
- every exported span carries the Belltower `session.id` correlation attribute
- the collector upload path uses the same canonical stored events; it does not build a separate telemetry model
- if `PHOENIX_COLLECTOR_ENDPOINT` is set, the push path can omit the endpoint argument
- if `PHOENIX_API_KEY` is set, the push path uses it for `Authorization: Bearer ...`
- if `PHOENIX_CLIENT_HEADERS` is set, those extra OTLP headers are merged into the upload request
- the TUI push command should prefer environment-backed auth rather than raw inline secrets, so API keys do not end up in the canonical session log
- when the collector returns OTLP `partial_success`, Belltower surfaces:
  - rejected span counts when the collector reports them
  - collector warnings even when the request is fully accepted
- if the collector returns a non-empty but unreadable `200 OK` success body, Belltower keeps the upload as successful but surfaces a warning so live validation remains diagnosable

Portable-session direction:

- treat the current `/export legacy-bundle` JSON response as a stale
  transitional export/inspection surface; the portable sharing format is the
  `session.bt` shape documented in
  [`../architecture/session-bundles-and-trace-sync.md`](../architecture/session-bundles-and-trace-sync.md),
  not the old JSON shape
- keep the core bundle tight: manifest, canonical events, raw chunks, payloads,
  artifacts, and checksums
- keep transcript, TUI history, OTLP/OpenInference, ShareGPT, HTML, and search
  indexes as derived projections rather than canonical bundle content
- preserve `event_id`, treat SQLite `seq_id` and raw chunk row ids as local
  debug/display metadata, and validate portable `event_hash` values over
  documented canonical event bytes using `sha256:<lowercase-hex>` hashes
- keep `SessionExportResponse { content: String }` for textual/projection
  exports; `session.bt` export/import/validate/diff need distinct
  file/archive-aware protocol and CLI surfaces
- the first cutover slice exposes this locally as
  `belltower session export <session-id> --out <dir>` and
  `belltower session validate <bundle>`, followed by
  `belltower session import <bundle>` for local rehydration and
  `belltower session diff <a> <b>` for structural comparison. Local
  continuation uses `belltower session continue <bundle> [--event-hash
  sha256:...]` to create a branch from the default head or selected portable
  node. Private filesystem sync uses `belltower session push <bundle> --remote
  <dir>` and `belltower session pull --remote <dir> --session <id> --out <dir>`
  to exercise content-addressed bundles and compare-and-swap branch-head
  movement before HTTP archive transport lands.
- validation is closed: unknown bundle files, unreferenced checksum entries,
  missing checksum paths, malformed branch/event ranges, dangling raw refs, and
  mismatched branch-head tuples reject before import or sync
- import is atomic: a validation-passing bundle that fails during event
  denormalization or append leaves no partial session, branches, events, or raw
  chunks in the destination store
- continuation from an already-imported bundle recomputes the selected parent
  event hash from local canonical evidence; a matching `event_id` with different
  content is rejected rather than silently treated as the same node
- the manifest owns the full `SessionRecord`, not only the session id, so import
  can recreate the session boundary without inferring project root,
  connection/model defaults, settings revision, tool mode, or parent refs
- `bundle_hash` hashes the checksum tree over non-manifest bundle contents;
  the manifest records that hash and is validated for consistency without
  needing a circular checksum entry for itself
- include deterministic `ContentRef` mappings for raw chunks in the first
  bundle schema/export slice, not as a later migration
- add `belltower session validate <bundle>` before remote push/pull so external
  readers can verify a bundle offline
- add `belltower session diff <a> <b>` as the structural review surface for
  same-lineage updates, forks, replication attempts, and redaction previews.
  The local diff compares portable event hashes and branch/content inventories,
  not SQLite sequence ids or raw chunk row ids.
- add `belltower session continue <bundle> [--event-hash sha256:...]` as the
  local continuation surface. The resulting branch stores the selected external
  `SessionNodeRef` in the canonical `branch.created` event attributes and uses
  existing branch lineage bounds for context reconstruction.
- local rewind uses the same lineage model. A branch can be created from a
  selected in-session event boundary, and portable bundles preserve
  `parent_branch_id`, `parent_event_id`, branch heads, and branch-scoped event
  ranges so continuation/replay does not depend on local SQLite ids.
- add private `belltower session push` and `belltower session pull` surfaces for
  filesystem-backed remote sync. Push validates the bundle and advances the
  remote branch head only when the expected head matches; pull copies and
  validates a remote bundle but does not import or continue it implicitly.
- artifact modes are explicit bundle export modes: `trace-only`,
  `trace-plus-artifacts`, and `trace-plus-patches`. Explicit artifact paths must
  resolve inside the session project root; patch mode can add a Git diff patch
  artifact from the project root. These artifacts are canonical bundle content,
  while rendered papers, HTML views, and OTLP traces remain derived projections
  unless exported as explicit artifacts.
- use `SessionNodeRef`, `ContentRef`, `EventRange`, and `BundleArtifactRef` as
  the shared primitives for continuation, artifact materialization, subagent
  handoff, and future remote sync

Important rule:

- exporters read from the canonical session store
- exporters do not become alternate sources of truth
- runtime `tracing` instrumentation mirrors committed events, not bypasses them
- export, validate, import, diff, push, pull, share, and publish record compact
  operation summaries when they affect operator-visible truth; they do not write
  exported content previews back into the evidence boundary being exported

## Human Checkpoints

1. create a session with streamed output and at least one tool call
2. inspect structured events
3. inspect raw chunks
4. export the session as JSONL and HTML
5. export, validate, import, continue, and diff a `session.bt` bundle
6. preview OTLP/OpenInference spans with `/export otlp`
7. push OTLP/OpenInference spans with `/export otlp push [collector-endpoint] [project-name]`
8. inspect the uploaded trace in a consumer such as Phoenix

## Current Priorities

- stronger OI/OTel validation against a real external consumer such as Phoenix
- export and inspection views that make raw chunks, structured events, derived spans, and workflow lineage inspectable together
- explicit error taxonomy, so `/usage`, export, and failure rendering stay trustworthy as the harness grows
- replay- and dataset-ready export selectors over canonical stored history rather than one-session-only projections
