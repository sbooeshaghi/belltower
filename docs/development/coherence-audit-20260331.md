# Coherence Audit Log — 2026-03-31

This log records the discrepancies found during the 2026-03-31 Belltower coherence pass.

It is grouped by subsystem and severity, and it records the resolution taken in the same pass.

Severity meanings:

- `P1`: direct architecture/code conflict or a break in the active test surface
- `P2`: stale or misleading docs on important user-facing or architectural behavior
- `P3`: lower-risk wording drift or missing clarification

## Core Runtime and Telemetry

### `P1` Code drift: `bt-otel` test fixture no longer matched `SessionRecord`

- Issue: `bt-otel` tests still constructed `SessionRecord` without `tool_mode`.
- Source of truth: code and current core types.
- Resolution: updated the fixture to the current `SessionRecord` contract and restored the broader test surface.

## Tools and Approvals

### `P1` Code drift: `fetch` approval policy violated the tool architecture doc

- Issue: the architecture doc treated `fetch` as conditionally safe, but `FetchTool` still used `ApprovalRequirement::Never`.
- Source of truth: architecture doc plus target-goals expectation that remote actions must be governed truthfully.
- Resolution: `fetch` now uses conditional approval and moderate risk metadata, with a focused unit test.

### `P2` Docs stale: tool normalization doc still read like a pre-normalization migration plan

- Issue: the doc still described canonical renames and `plan` / `ask` / `inspect` / mode gating as future cleanup.
- Source of truth: code and current architecture.
- Resolution: rewrote the architecture doc sections to describe the normalized tool surface as current state and moved the “what remains” discussion to remaining metadata/runtime work.

### `P2` Public-surface cleanup: remove the empty `autonomous` session mode

- Issue: the public session-mode surface still exposed `autonomous` even though it behaved identically to `extended` and the architecture docs described autonomous capability growth as future work.
- Source of truth: architecture docs and current runtime behavior.
- Resolution: removed `autonomous` from the public `SessionToolMode` surface, updated the TUI `/mode` flow to `standard|extended`, and kept old stored `autonomous` values loading as `extended` in the session store.

## Operator Surfaces

### `P2` Docs stale: provider-output doc still implied transcript rerender toggles

- Issue: the inline TUI no longer supports local history-rerender toggles such as `/show thinking`; transcript history is append-only and richer detail comes through `inspect`.
- Source of truth: TUI command registry and current operator behavior.
- Resolution: updated the architecture doc to describe immutable transcript history and inspection-first drill-down instead of `/show thinking`.

### `P2` Docs stale: current sprint still centered the just-completed operator-hardening work

- Issue: the active sprint doc still described TUI and readiness work as the primary in-flight implementation focus.
- Source of truth: current codebase and active alignment pass.
- Resolution: rewrote the sprint doc around the coherence audit and release-alignment phase.

## Protocol, MCP, and Public Control Plane

### `P2` Docs stale: protocol and MCP docs under-described the current public surface

- Issue: the docs did not adequately reflect `/models/connections`, `/mcp reload`, the TUI `/mcp` command, and `inspect { query: "mcp" }`.
- Source of truth: server routes, client methods, TUI command surface, and inspection tool contract.
- Resolution: updated subsystem docs to describe the current public surfaces and the actual MCP lifecycle states.

### `P2` Docs stale: MCP status language implied states not exposed by the current public model

- Issue: MCP docs spoke about “degraded/unreachable” as separate operator states, while the public status model is `Configured`, `Ready`, or `Degraded`.
- Source of truth: `McpServerStatus` in `bt-core`.
- Resolution: tightened the docs to the actual public state model and current operator surfaces.

## Planning and Sequencing

### `P2` Docs stale: roadmap and parity docs still recommended already-completed next slices

- Issue: `platform-roadmap.md` and `subsystem-parity-matrix.md` still pointed to already-landed TUI/startup/model-readiness work as the near-term order.
- Source of truth: current implementation state plus the new source-of-truth matrix.
- Resolution: rewrote the near-term sequencing sections to reflect post-hardening work instead of historical recommendations.

## Documentation Governance

### `P2` Missing explicit truth policy for docs

- Issue: the repo had an implicit distinction between architecture, subsystem, planning, development, and reference docs, but not an explicit matrix for resolving conflicts.
- Source of truth: this audit pass requirement.
- Resolution: added `source-of-truth-matrix.md` and linked it from the docs entrypoint.

## Intentional Non-Fixes

These were reviewed and left intentionally unchanged:

- model-family aliasing remains deferred
- structured output remains implemented but low-profile in operator UX
- dataset/replay remains architectural and planning work, not a current runtime surface

Those are not coherence bugs; they are deliberate scope boundaries.
