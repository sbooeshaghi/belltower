# Harness Interop Plan: Belltower as the Hub

Date: 2026-07-25. Status: agreed direction, pre-implementation. Supersedes
the build-order sketch in
[foreign-harness-interop-study-20260724](./foreign-harness-interop-study-20260724.md)
(the study's analysis stands; this document is the decision).

Decisions recorded here:

- **Trajectory is out of scope.** No trajectory export, no upstream adapter.
  The study's §8 analysis is kept for its schema-formalization lessons only.
- **xtend is a predecessor, not a dependency.** It was built before the
  event-log-as-ground-truth worldview and shuttles native transcripts with
  pairwise converters. Belltower absorbs that responsibility natively; xtend
  and the xtensions traces become reference implementation and test
  fixtures, not maintained machinery.

## The inversion

xtend's model is pairwise: claude↔codex converters over native transcripts,
N×N as harnesses grow. With the event log as ground truth, the coherent
model is **hub-and-spoke**: the Belltower canonical event model is the
interchange representation, and each foreign harness gets one adapter pair
against it —

- **ingress**: native session → canonical events (import, full fidelity
  preserved as evidence),
- **egress**: canonical events → native resumable session (handoff).

"Claude Code and Codex interface with Belltower" then means three concrete
things, in increasing order of ambition:

1. Their sessions can be **adopted** into the Belltower store — Belltower
   becomes the durable system of record for all agent work on the machine,
   regardless of which harness produced it. Everything Belltower already has
   (turn projections, search, lineage, inspection, bundles, mesh
   continuation) applies to adopted history for free.
2. Any Belltower session — native or adopted — can be **handed off** as a
   real resumable session in the harness of the recipient's choice.
3. Round trips preserve **lineage as a DAG in canonical truth**: work
   started in Claude Code, continued in Belltower, handed to Codex, and
   returned still has one ancestry chain, via `parent_external_ref`
   node refs. No other tool in this space maintains cross-harness lineage;
   this is the differentiator.

The horizon item beyond trace-level interop (explicitly not in this plan's
phases): foreign harnesses as **live clients** — a Belltower MCP surface so
a Claude Code session can spawn/message/inspect Belltower agents and have
its operator actions recorded canonically in real time. The claude-cli
provider already proves the inverse direction. Design later; nothing in the
phases below forecloses it.

## Phases

### Phase 0 — Formalize the contract (first, and small)

- `schema/belltower-event-v1.schema.json` — the `EventEnvelope` and every
  payload kind, strict (`additionalProperties: false`, exact serialized
  enum casing), plus `schema/belltower-bundle-manifest-v1.schema.json`.
- Generated from the Rust types (schemars); CI drift gate requires
  regeneration to be byte-identical. Code stays the source of truth.
- Document the semantic-vs-transport field split that `event_hash`
  canonical bytes already imply, so external validators can be written
  against the schema alone.
- Exit test: an external (non-Rust) script validates a real exported
  `session.bt` using only the published schemas.

This is what makes "interface with Belltower" possible without reading
Rust, and every later phase consumes it.

### Phase 1 — Ingress: adopt Claude Code and Codex sessions

- New seam (`bt-interop` crate or `bt-session::interop`):
  `ForeignSessionReader` produces an **import plan** — session record,
  ordered synthetic canonical events, per-event raw-line refs, child
  sessions — which feeds the *existing* atomic bundle-import write path.
  One write path, one validator, for both `session.bt` imports and foreign
  adoption.
- Claude Code reader first (richest format; the operator's own history;
  sidecar subagents → related sessions with the spawn `tool_use` id as the
  linkage). Codex reader second (thinner, same seam).
- The study's §4a non-negotiables are the acceptance bar: synthesized logs
  pass the same coherence invariants as live logs (validator-gated import);
  honest terminalization of dangling structures; provenance as explicit
  external-ref attributes; every foreign line preserved as the raw chunk of
  the event synthesized from it; fresh Belltower ids everywhere.
- CLI: `belltower session import --from claude|codex|auto <path|session-id>`
  and the wedge use case, bulk adoption:
  `belltower adopt --from claude-code [--project <dir>]` scanning the
  native store. Adopted sessions are ordinary sessions — resumable in
  Belltower via the existing continuation seam.

### Phase 2 — Egress: hand off to Claude Code and Codex

- `ForeignSessionWriter`: canonical events → native transcript,
  **deterministic** (same events, same bytes — required for stable line
  citations and append-only updates).
- Fidelity upgrade over xtend: because canonical truth is richer than
  either native format, egress renders **native tool blocks**
  (`tool_use`/`tool_result` with real call ids for Claude Code;
  `function_call`/`*_output` response items for Codex) instead of
  bracketed one-line folds. Only what has no native representation (mesh
  messages, approvals, policy decisions) folds into bracketed notes, with
  the provenance preamble at the top.
- Placement is the adapter's job (xtend's three rewrites, done natively):
  fresh target-tool session id, cwd retarget, tool-native file placement +
  index registration. CLI:
  `belltower session handoff <id> --to claude|codex [--cwd <dir>]` →
  prints the `claude --resume <id>` / `codex resume <id>` command.
- The egress boundary is recorded in the source session's log (an export
  event carrying the boundary `SessionNodeRef` and target-tool session id)
  so returns can attach (Phase 4).

### Phase 3 — Bundles that carry whole workflows, and sharing

- **Tree bundles**: export a session graph (parent + selected descendants)
  as one `session.bt` unit — per-session event logs plus a graph manifest
  reusing `SessionNodeRef` for parent links and spawn-call refs. Import
  recreates related-session rows so `list_agents`, wake, and settlement
  survive the trip. Without this, exporting a mesh parent is an incomplete
  story.
- **HANDOFF.md** adopted as a first-class convention in the bundle
  *wrapper* (registry layer), agent-written at export, stub-refused at
  share time — outside the checksummed core so the human note can be
  edited without invalidating evidence.
- **Registry**: a git repo of bundle directories, PR-gated and append-only
  (the xtensions conventions, kept), with `belltower session push/pull`
  as transport. Optionally include an egressed native transcript beside
  the bundle as a derived courtesy artifact for recipients without
  Belltower.
- **Redaction ordering fixed by construction**: redaction is an
  export-time policy applied before hashing; a published bundle's hashes
  cover the redacted bytes. Never redact-after-hash.

### Phase 4 — Round-trip continuity

- Re-importing a continued handoff recognizes the egress provenance (the
  preamble/external refs written in Phase 2) and attaches the new work as
  a **continuation branch** whose `parent_external_ref` is the egress
  boundary node — not a fresh unrelated session. Divergent continuations
  in different harnesses become siblings under one ancestor, inspectable
  with the existing diff/lineage tooling.
- Exit test: claude → belltower → codex → belltower round trip yields one
  DAG, every node's log passing validation, with the foreign segments'
  raw lines intact as evidence.

## Sequencing rationale

Schemas first because they are cheap and every consumer (adapters,
external validators, future viewers) depends on them. Ingress before
egress because adoption delivers immediate value on day one (the
operator's existing history becomes Belltower-native) and forces the
validator gate to mature against real messy transcripts before we emit
anything. Tree bundles before round trips because scientific workflows are
trees, and a handoff that silently drops the subagent evidence would
betray the ground-truth premise.

## Explicitly out of scope

- Trajectory (any direction) — user decision, 2026-07-25.
- Live MCP client surface — horizon, design after Phase 2.
- Merge semantics between divergent continuations — branches and diffs
  only, per the bundle architecture's non-goals.
- Maintaining xtend — its example bundles (and the xtensions traces)
  become fixtures for Phase 1 readers; its conversion decisions are cited
  precedent, not code we run.
