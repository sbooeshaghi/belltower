# Foundation Acceptance Log - 2026-04-24

This log records operational foundation gates that are intentionally not
always-on in default CI because they require local loopback processes, tmux, or
live provider credentials.

## Commands Run

```bash
make acceptance
```

Result: passed on commit `c71cd29`.

Scope: deterministic foundation acceptance, including the default TUI
deterministic gate.

```bash
BELLTOWER_TUI_ACCEPTANCE_TURNS=500 bash scripts/tui_acceptance.sh tmux
```

Result: passed on commit `c71cd29`.

Observed evidence:

- TUI launched against a temporary `bt-server` and local streaming mock
  provider.
- Script drove 500 long-session prompt turns plus one approval-producing turn.
- `/inspect session` reported `Turns: 502`, `Messages: 1004`,
  `Tool calls: 1`, `Approvals: 1 total, 0 pending`, `Raw chunks: 1506`,
  and `Controls: cancel_requested=false pending_steer=0`.
- `/compact` completed successfully.
- `/export jsonl` completed successfully and reported `bytes=5464489`.

```bash
BELLTOWER_LIVE_ACCEPTANCE=1 bash scripts/live_acceptance.sh local
```

Result: passed on commit `c71cd29`.

Observed evidence:

- The script created a temporary-server session on the configured `local`
  connection.
- The first turn returned the expected token
  `belltower-live-local-ok`.

```bash
BELLTOWER_LIVE_ACCEPTANCE=1 bash scripts/live_acceptance.sh cloud
```

Result: passed on commit `c71cd29`.

Observed evidence:

- The script created a temporary-server session on the configured `chatgpt`
  connection.
- The first turn returned the expected token
  `belltower-live-cloud-ok`.

## 2026-05-12 Closeout Refresh

```bash
make acceptance
```

Result: passed on the closeout worktree before commit.

Scope: deterministic foundation acceptance, including server/API contracts,
readiness, pricing, control persistence, inspection, replay, compaction, MCP
degraded behavior, exports, and deterministic TUI tests.

Not run: live provider gates and tmux smoke. Those remain explicit opt-in
operational gates because they require credentials or an interactive terminal
session.

```bash
make file-size
```

Result: passed on the closeout worktree before commit.

## Remaining Operational Gate

M9 dogfood remains open. It requires one full work week of real Belltower-on-
Belltower use and a dated dogfood log with the friction dimensions from
`docs/planning/self-hosting-foundation-plan.md`.

## 2026-05-20 Scientific Workflow Closeout Refresh

```bash
make acceptance
```

Result: passed on the scientific workflow closeout worktree before commit.

Scope: deterministic foundation acceptance, including server/API contracts,
readiness, pricing, control persistence, inspection, replay, compaction, MCP
degraded behavior, export fidelity, deterministic TUI tests, and portable
`session.bt` bundle coverage.

Additional coverage added to the default gate:

- Exact-test guards now fail the runner when a named acceptance test is stale
  instead of silently passing with `0 tests`.
- Export fidelity uses the current
  `session_export_returns_legacy_bundle_jsonl_html_sharegpt_and_otlp` server
  contract test.
- Portable session bundles are checked for offline validation,
  import round-trip, explicit artifact refs, tamper rejection, raw-chunk content
  integrity, same-lineage and fork diffs, bounded continuation from a mid-node,
  event-boundary branch forks, and filesystem CAS push/pull semantics.

Not run: live provider gates and tmux smoke. Those remain explicit opt-in
operational gates because they require credentials or an interactive terminal
session.
