# Acceptance Milestone Runner

`make acceptance` runs `scripts/acceptance.sh`.

The runner is the foundation-plan Direction 4.4 gate. It is intentionally
honest about scope: it executes deterministic milestone proxies that do
not need real provider credentials or network access, and it prints
explicit skips for the live local/cloud happy paths until a later live
acceptance lane configures them.

## Current Coverage

- `M0` install and health: server health/capability/auth-boundary tests,
  launcher tests, readiness tests, and TUI doctor/status fixtures.
- `M1` local model happy path: skipped unless a live local backend is
  configured in a follow-on lane.
- `M2` cloud provider happy path: skipped unless real provider
  credentials and network access are configured in a follow-on lane;
  curated pricing/readiness model coverage runs now.
- `M2.1` control-plane and protocol health: typed error envelopes,
  cross-process SSE replay, and cancel/steer persistence.
- `M3` tool use and approvals: built-in tool approval round-trip,
  durable operator commands, and canonical inspection reconstruction.
- `M4` replay and raw chunks: session event/raw-chunk persistence and
  SSE replay.
- `M5` branching and tree continuity: branch activation, child-session
  lineage, and workflow inspection.
- `M6` compaction correctness: runtime-owned compaction route smoke.
- `M7` MCP integration: MCP inventory/reload/degraded behavior plus the
  shared inspection contract.
- `M7.1` instructions and skills: durable turn instruction provenance
  and protocol fixture stability.
- `M8` telemetry/export: bundle, JSONL, ShareGPT, HTML, and OTLP bundle
  shape export coverage.
- Scientific workflow portable trace gate: `session.bt`
  export/validate/import/diff/continue, artifact refs, raw-chunk integrity,
  event-boundary branch forks, closed bundle inventory validation, atomic import
  rollback, existing-session parent-hash verification, and filesystem CAS
  push/pull.

`M4.1` remains the dedicated long-session TUI gate in Direction 5.6, and
`M9` remains the dogfood-week gate in Direction 5.7.
