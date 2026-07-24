# Foreign-Harness Session Interop Study (xtend / xtensions / trajectory)

Date: 2026-07-24 (trajectory addendum same day). Status: study only — no
implementation. Sources: the
`sbooeshaghi/xtend` and `sbooeshaghi/xtensions` repos (read at HEAD),
Belltower's `session.bt` architecture
([session-bundles-and-trace-sync](../architecture/session-bundles-and-trace-sync.md))
and its implemented exporter/importer/continue/diff/push/pull surface in
`bt-session::export`/`sync` and the launcher `session` subcommands.

Goal under study: sessions importable into Belltower from other harnesses,
resumable here; Belltower sessions exportable so other harnesses can resume
them; every import producing a coherent canonical event log; adapters per
harness where formats differ.

## 1. What xtend is

xtend is a zero-dependency Python CLI + Claude Code skill + MCP server that
packages a **Claude Code** or **Codex** session into a trace bundle (an
"xtension") inside a git repo, and imports bundles back into either tool.

Mechanics that matter for Belltower:

- **Bundle shape** (`"xtend": 1`): `manifest.json` (tool, tool_version,
  session_id, `trace_id`/`parent_trace_id` lineage pair, exporter identity,
  project git state, secret-scan status, title/description/tags),
  `session.jsonl` (the tool's native transcript, byte-faithful modulo
  redaction), `sidecar/` (Claude Code subagent transcripts +
  `agent-*.meta.json` with `agentType`, `toolUseId`, `parentAgentId`,
  `spawnDepth`; plus tool-result assets like PDF page images), optional
  `artifacts/`, and a human `HANDOFF.md`.
- **Import = three rewrites**: fresh session id, cwd retargeted (metadata and
  transcript text), file placed where the target tool's session discovery
  expects it (`~/.claude/projects/<cwd-slug>/<id>.jsonl` /
  `~/.codex/sessions/YYYY/MM/DD/rollout-…jsonl` + `session_index.jsonl`).
  Resume is then the tool's own native `--resume`.
- **Cross-tool conversion is deliberately lossy-but-honest**: conversation
  text verbatim; tool calls/results folded into bracketed one-line summaries
  (`[tool call: …]`, `[tool result: …]`, capped at 300/700 chars); foreign
  system/environment payloads dropped (the target tool injects its own); an
  explicit preamble line tells the resumed model it is continuing an imported
  conversation. Unknown lines are copied byte-faithfully so the tool degrades
  gracefully as formats evolve.
- **Lineage**: exports stamp `trace_id`; imports record it, and a later
  export from the continued session stamps `parent_trace_id` — merged
  history forms a DAG across people and tools.
- **Privacy is a first-class gate**: regex secret scan blocks export;
  optional local PII model (`--deep`) produces a reviewable replacement map;
  `share` refuses template-stub handoffs. PR review is the human gate.

## 2. What xtensions is

A registry repo: every directory under `xtensions/` is one bundle. The
conventions are the contract:

- **Append-only**: `export --update` may only extend a transcript; merged
  xtensions are never edited. This makes **line-level citations stable**
  (`session.jsonl:L618-L622` in a paper stays valid forever) — the property
  the whole scientific-provenance story hangs on.
- **Manifests are the index**: a browser site just walks
  `xtensions/*/manifest.json`; no separate catalog.
- **Work arrives by PR**, so the repo history is a reviewable record of the
  work itself.

## 3. What Belltower already has

The `session.bt` bundle (implemented: export/validate/import/diff/continue/
push/pull as `belltower session …` subcommands) is a *stronger* portability
primitive than an xtension along every integrity axis:

- content-derived identity (`event_hash = sha256` over canonical portable
  bytes; local seq ids explicitly non-portable), checksummed closed
  inventory, atomic import, structural diff, fork-from-any-node continuation
  via `SessionNodeRef` + `parent_external_ref` on `branch.created`,
  append-only push/pull sync.
- The canonical event taxonomy is far richer than either foreign transcript:
  explicit turn boundaries, full tool lifecycle with approvals and policy
  decisions, context manifests, instruction provenance, raw provider chunks,
  related-session (mesh) messaging with atomic dual commits.

And one relevant weakness: **`session.bt` is single-session**. A parent's
bundle carries its own related-message events but not its children's logs.
xtend's Claude Code bundles carry the whole subagent tree as sidecars. For
Belltower's mesh trees (siblings, cousins, settlement replies) a
single-session export is an incomplete story.

## 4. Coherence analysis — the three interop directions

### 4a. Foreign → Belltower (import + resume here)

Belltower's stated non-goal — "no foreign trace format as Belltower's source
of truth" — is the right constraint and dictates the adapter's job:
**synthesize canonical events from the foreign transcript**, don't store
foreign lines as truth. Per harness:

- **Claude Code jsonl** (`type: user|assistant|summary|queue-operation`,
  message content blocks `text|tool_use|tool_result|thinking`, `parentUuid`
  chains, `isSidechain`, sidecar subagent files): maps surprisingly well
  onto the canonical taxonomy. A user line opens a turn
  (`message.appended` + `turn.started`); assistant blocks become
  `completion.*` evidence and `message.appended`; `tool_use`/`tool_result`
  pairs become `tool.call.requested`/`tool.execution.finished` (call ids are
  present: `toolu_…`); the assistant line that ends without tool calls closes
  the turn (`turn.finished { completed }`). Sidecar subagents map onto
  **related sessions**: `parentAgentId`/`spawnDepth` give the lineage,
  `toolUseId` links the spawn to the parent's tool call. `summary` lines map
  to a context-compaction record. `queue-operation` lines map to queued-input
  events or are dropped as non-canonical UI noise (decide explicitly).
- **Codex rollout jsonl** (`session_meta` + `response_item` payloads
  `message|function_call|local_shell_call|*_output`, plus `event_msg` UI
  lines): same synthesis with a thinner tool story; `event_msg` lines are
  derived-view noise to skip. Codex subagent child threads aren't bundled by
  xtend yet — accept that as a known upstream gap.

Non-negotiables for coherence, learned from this week's work:

1. **Every synthesized log must pass the same invariants the live log
   passes** (the audit I ran: monotone seqs, started/finished pairing, tool
   terminal closure, requested→chunks→finished per turn-scoped llm call,
   dual-commit message pairing, projection==replay). The importer should run
   the validator as its last step and refuse incoherent output. Foreign
   transcripts are messy (interrupted turns, dangling tool calls); the
   adapter must terminalize honestly — e.g. a dangling `tool_use` becomes a
   terminal error result with an `imported.interrupted` finish reason, never
   a silently open turn.
2. **Provenance is recorded, not implied**: an imported session's
   `session.started`/record carries external refs (source tool, source
   session id, xtend `trace_id`, bundle hash) via event attributes — the
   `parent_external_ref` pattern already exists for exactly this. Fresh
   Belltower ids everywhere; foreign ids live only in provenance attrs.
3. **Raw durability transfers**: the original foreign jsonl line for each
   synthesized event is preserved as that event's raw chunk (the same
   `raw_chunk.persisted` + content-ref machinery the claude-cli provider
   uses). This is the escape hatch that makes the synthesis auditable and
   re-runnable when the adapter improves: canonical events are derived; the
   verbatim foreign line is evidence.
4. **Resume is just Belltower resume**: once imported, continuing is the
   existing branch-continuation seam (`session continue`), with context
   assembled from the imported messages. No special "foreign resume" mode.
   Fidelity choice at import: `--fidelity full` (tool events synthesized,
   default) vs `--fidelity folded` (xtend-style bracketed summaries) for
   hostile/unknown formats.

### 4b. Belltower → foreign (others resume our sessions)

The inverse adapter is easier and xtend has already proven the pattern:
conversation verbatim, tool activity folded into bracketed summaries,
preamble line explaining the import. Belltower's exporter renders canonical
events → the target tool's native transcript (claude jsonl with a proper
`parentUuid` chain, or codex `response_item`+`event_msg` pairs), places
nothing itself — emitting an **xtension bundle** (`tool: "belltower"` in the
manifest, `session.jsonl` in the *target* tool's format or in ours with a
converter, `HANDOFF.md`, sidecars for the session tree) and letting xtend do
placement/lineage/PR. Two integration options, not exclusive:

- Belltower emits native xtension bundles (`session export --to xtension`),
  so the existing xtend/xtensions ecosystem (share, redact, registry, PRs)
  works unchanged; and/or
- xtend grows a `belltower` tool adapter (it's ~200 lines per direction in
  that codebase; its architecture expects new tools). The `session.bt`
  bundle can ride inside the xtension directory as the high-fidelity
  artifact next to the converted `session.jsonl` — xtensions' manifest-walk
  ignores files it doesn't know, by design.

### 4c. Belltower ↔ Belltower (already mostly built)

`session.bt` + push/pull covers same-harness sharing with better guarantees
than xtend (hashes, atomic import, fork-from-node). Two gaps to close for
parity with the mesh work:

- **Tree bundles**: export a session *graph* (parent + descendants, or any
  subtree) as one bundle — per-session `events.jsonl` sub-bundles plus a
  graph manifest reusing `SessionNodeRef` for the parent links and the
  spawn-tool call refs (`toolUseId`-equivalent: the spawn `tool.call`
  event). Import recreates the related-session rows so `list_agents`, wake
  and settlement semantics survive the trip. Without this, an exported mesh
  parent references children the recipient doesn't have.
- **HANDOFF.md**: session.bt has no human note. Adopt xtend's convention
  verbatim (agent-written at export, refused-if-stub at share time) — it's a
  registry/manifest concern, so it belongs in the xtension wrapper layer,
  not inside the checksummed core bundle.

## 5. Adapter architecture recommendation

- One new seam: `bt-interop` (crate or `bt-session::interop` module) with a
  small trait per direction —
  `ForeignSessionReader` (foreign file(s) → normalized import plan:
  session record, ordered synthetic events, raw-line refs, child sessions)
  and `ForeignSessionWriter` (canonical events → foreign transcript). Two
  implementations each for claude-code and codex. The import plan feeds the
  *existing* atomic bundle-import path so foreign imports and session.bt
  imports share one write path and one validator.
- Operator surface: `belltower session import --from claude|codex|auto
  <path|session-id>` (auto-detect by shape), `belltower session export
  --to xtension [--tool claude|codex] --out <dir>`. Skill-level verbs can
  mirror xtend's ("export this session for handoff…").
- Keep xtend as the cross-tool userland: don't rebuild share/redact/PR/
  registry in Rust. Belltower's job is (a) speak its own canonical truth,
  (b) read/write the two foreign formats, (c) emit/consume xtension
  bundles. Redaction interplay: xtend's scan operates on `session.jsonl`
  text; Belltower's `RedactionPolicy` operates on the content-addressed
  bundle. For published xtensions containing a `session.bt`, the redaction
  must happen *before* hashing (export-time policy), or the bundle ships
  unredacted — this ordering needs to be explicit in the eventual design.

## 6. Known gaps and risks (ranked)

1. Dangling/interrupted foreign structures (open tool calls, mid-stream
   truncation, compaction summaries) — the adapter's honesty rules above.
2. Mesh/tree export gap in session.bt (4c) — required before "export a
   scientific workflow" is a complete story.
3. Claude Code sidecar richness (tool-result assets, thinking blocks with
   signatures, queue-operations, permission modes) — decide per record:
   canonical event, raw-only evidence, or dropped; write the mapping table
   down as part of the adapter spec before coding.
4. Codex child threads not bundled upstream — document as a known limit.
5. Timestamps: foreign lines carry wall-clock; canonical events order by
   commit sequence. Import must preserve foreign order even when timestamps
   are non-monotonic (order by file position, keep timestamps as recorded).
6. Append-only update flow (`export --update` equivalence): re-exporting a
   continued Belltower session into the same xtension must only extend
   `session.jsonl` — the event log's append-only nature makes this natural,
   but the converter must be deterministic (same events → same lines) or
   line-citation stability breaks. Determinism is therefore a hard
   requirement on the writer adapters.

## 7. Suggested build order (when implementation is approved)

1. Claude Code **reader** + import-plan → atomic import + validator gate
   (biggest value: the user's own history becomes resumable in Belltower;
   sidecar subagents → related sessions).
2. Xtension **writer** (belltower → claude-format xtension incl. HANDOFF
   flow), deterministic renderer.
3. Codex reader/writer (thinner formats, same seams).
4. Tree bundles for session.bt (mesh-complete same-harness sharing).
5. xtend upstream PR: `belltower` tool support, so `xtend extend <name>
   --tool belltower` resolves naturally.

With the trajectory addendum below, two items join this list:

0. **Formalize the Belltower schemas** (moves ahead of everything — it
   unblocks external consumers, upstream adapters, and validator reuse; see
   §8d).
6. **Trajectory-v1 export projection** (+ optional upstream
   `source: "belltower"` adapter PR; see §8b).

## 8. Addendum: letta-ai/trajectory (released 2026-07-23)

Reviewed at HEAD: the npm/PyPI `@letta-ai/trajectory` package, its two
JSON Schemas, eight shipped harness adapters (claude-code, codex,
letta-code, openclaw, openhands, pi, hermes, deepagents), the
`CANONICAL.md` cloud-ingestion contract, and the `prompts/add-source.md`
agent prompt for writing new adapters.

### 8a. What it is — two layers, one lesson each

**trajectory-v1** is a deliberately tiny, token-efficient record format for
*agents reading past sessions*: five roles (`meta`, `user`, `reasoning`,
`assistant`, `tool`), tool results linked by `tool_call_id`, long outputs
optionally truncated, harness bookkeeping dropped. ~5x token reduction vs
native transcripts. The schema is strict: `additionalProperties: false`
everywhere, a conditional rule that `content` is null iff `tool_calls`
present, `$id`-versioned. Explicitly positioned *against* full-fidelity
formats (Harbor ATIF) — experience data for memory formation, not replay.

**canonical-v1** is their cloud ingestion contract layered on top, and it
convergently reinvents most of `session.bt`'s identity discipline: stable
source-record ids that are never transport-order-derived, a
`record_hash` (transport bytes) vs `content_hash` (semantic bytes,
timestamps excluded) split, lexicographic order keys instead of positional
identity, append-only source "generations" where truncation means a new
generation, dual provenance stamps on every derived row
(`normalizer_version` + `schema_version`), and **quarantine-over-fallback**
failure modes (`source_group_conflict` fails the upload rather than
minting bad identity). Belltower's bundle design made the same calls
independently — good evidence both are right.

Ecosystem signal: eight harnesses already covered (including hermes),
adapters are written *by coding agents* from a maintained prompt, and
letta-code's memory "dreaming" consumes them across harnesses. This format
is becoming the lingua franca for agent experience data.

### 8b. Relevance to Belltower — what to adopt

1. **Emit trajectory-v1 as an export projection.** Canonical events →
   trajectory records is a small deterministic fold (meta from the session
   record; `message.appended`/completion evidence → user/assistant/
   reasoning; tool lifecycle → assistant tool_calls + tool records keyed by
   the existing call ids). It joins Belltower sessions to every trajectory
   consumer — letta's dreaming, cross-harness indexing, memory agents —
   at projection cost, like the ShareGPT/HTML exports. Related-session
   trees flatten to one trajectory per session (the format has no tree
   concept; the spawn tool calls and settlement results already appear as
   ordinary records, which is honest).
2. **Upstream `source: "belltower"` adapter, second.** Their normalizer
   contract takes one transcript string with no filesystem/SQLite access,
   so the upstream adapter needs a stable *file* to parse — either the
   trajectory files Belltower emits (trivial adapter) or the
   `events.jsonl` inside a `session.bt` bundle (full adapter, requires the
   published event schema below). Their `add-source.md` prompt makes this
   a one-session contribution.
3. **The memory direction (later, but real).** The blog's `/init` pattern —
   bootstrap an agent's memory from every harness's local sessions — is
   directly relevant to Belltower-as-lab-notebook: a Belltower memory/
   consolidation agent could read trajectory-normalized experience from
   Claude Code, Codex, and Belltower itself. Nothing to build now; the
   export projection (item 1) is the prerequisite.

### 8c. What not to do

- **Do not import trajectory files as resumable sessions.** They are too
  lossy for §4a's coherence rules (no turn boundaries, truncated results,
  dropped sidechains — their claude-code adapter *drops* subagent records
  that our importer must map to related sessions). Import stays on native
  transcripts; trajectory is an output/learning format.
- **Do not let a trajectory-shaped record model leak into the canonical
  taxonomy.** Five roles is the right size for experience data precisely
  because it discards what Belltower's log exists to preserve (approvals,
  policy decisions, mesh messaging, provenance).

### 8d. Schema formalization (endorsed, with a concrete shape)

The suspicion prompting this review is correct, and trajectory shows the
payoff pattern. Belltower's canonical shapes exist only as Rust/serde
definitions; nothing outside the workspace can validate a bundle or build
an adapter without reading Rust. Proposal:

- Publish versioned JSON Schemas as repo artifacts:
  `schema/belltower-event-v1.schema.json` (the `EventEnvelope` and every
  payload kind, strict per-kind: `additionalProperties: false`, required
  fields, enum casing as serialized — note the lowercase snake_case enums
  that already bit our own test scripts) and
  `schema/belltower-bundle-manifest-v1.schema.json` (manifest, node refs,
  content refs, event ranges, checksums).
- **Generate from the Rust types** (schemars) so code remains the source of
  truth, with a CI drift gate: regenerating must produce the committed
  schema byte-for-byte. Hand-maintained schemas fork; generated ones
  cannot.
- Stamp derived outputs the canonical-v1 way: every projection/export
  records the producing version and the schema version it targets
  (bundle manifests already carry producer info; extend the discipline to
  trajectory/ShareGPT/OTLP projections).
- Adopt their semantic-vs-transport hash distinction explicitly in the
  bundle docs: `event_hash` is already computed over canonical bytes that
  exclude local fields; documenting which fields are *semantic* (survive
  re-serialization) versus *transport* makes cross-implementation
  validation tractable.
- Version bump policy: schema version changes only on contract changes,
  never on packaging — their `CANONICAL_SCHEMA_VERSION` vs
  `NORMALIZER_VERSION` split, which maps cleanly onto
  `SESSION_BT_SCHEMA_VERSION` vs crate versions.

This is also the enabling move for §4/§7: xtend's `belltower` tool
support, trajectory's `belltower` source, and any third-party xtension
viewer all consume the published schemas instead of reverse-engineering
the store.
