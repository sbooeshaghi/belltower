# Foreign-Harness Session Interop Study (xtend / xtensions)

Date: 2026-07-24. Status: study only — no implementation. Sources: the
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
