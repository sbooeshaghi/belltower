# Belltower Development Rules

## First Read

If the task is not already narrow, read these first:

1. [`docs/architecture/overview.md`](./docs/architecture/overview.md)
2. [`docs/planning/target-implementation-goals.md`](./docs/planning/target-implementation-goals.md)
3. [`docs/planning/subsystem-parity-matrix.md`](./docs/planning/subsystem-parity-matrix.md)
4. [`docs/development/pattern-catalog.md`](./docs/development/pattern-catalog.md)
5. [`docs/development/developer-guidelines.md`](./docs/development/developer-guidelines.md)

Then read the relevant subsystem doc and the crate entrypoints you plan to change.

## Execution Contract

- Treat this file as the fast entrypoint. The deeper architecture,
  subsystem, planning, and development docs remain authoritative for
  their own truth tiers.
- Plans define outcomes, seams, invariants, and exit tests. They are not
  line-by-line implementation recipes. Choose the internal structure that
  best preserves the stated boundaries.
- If a task needs a wider write set, a public shape change, or an
  invariant change, stop and update the plan or architecture docs before
  continuing.
- Resolve doc/code disagreements through
  [`docs/development/source-of-truth-matrix.md`](./docs/development/source-of-truth-matrix.md).
  Code and focused tests describe current behavior; architecture docs
  describe long-lived invariants.
- Keep docs and code moving together. When an implementation changes a
  subsystem boundary, operator contract, event shape, protocol DTO, or
  durable behavior, update the relevant docs in the same slice.

## Operating Principles

- The harness is the product. Long-term leverage comes from orchestration,
  tools, context, memory seams, telemetry, evaluation, replay, and operator
  control, not from any single model provider.
- Complexity must pay rent. Add complexity only when it improves safety,
  observability, recoverability, extensibility, performance, or operator
  leverage against real workloads and failure modes.
- Schemas and tool interfaces are infrastructure. Events, DTOs, tool inputs,
  tool outputs, approvals, context bundles, and validation rules strongly
  shape reliability and model behavior.
- Context is a budget. Prefer high-quality, reconstructable context over
  broad retrieval, stale memory, repeated instructions, hidden prompt text,
  or excessive tool chatter.
- Observability enables automation. Build durable telemetry, inspection,
  replay, regression checks, and cost accounting before adding more
  autonomous behavior.
- Engineer for recoverability. LLM systems are probabilistic; use typed
  state, validation, constrained execution, explicit failure records, and
  resumable workflows rather than pretending determinism is guaranteed.
- Design for model evolution. Keep orchestration and provider seams thin,
  version public shapes carefully, and avoid encoding assumptions tied to
  one current model's behavior.

## Architecture Rules

- Do not bypass `bt-protocol` or `bt-client` for TUI or launcher convenience.
- Do not bypass `bt-session` for telemetry, replay, or export logic.
- Do not add hidden local execution paths that skip the server/runtime boundary.
- Do not treat credential presence as provider readiness when a real probe is possible.
- Raw provider output durability is required for feature-complete provider integrations.
- OpenInference-aligned session telemetry is a core invariant, not an optional add-on.
- Operator-visible actions that affect session truth must flow through a
  named operation or tool path and be recorded in durable session history
  and telemetry. This includes slash commands, approvals, asks, MCP
  operations, agent tool calls, and human-invoked tool operations.

## Multi-Agent Work

- Use separate branches and worktrees for parallel writer agents.
- One writer owns a hotspot at a time. Hotspots include runtime
  orchestration, server routes, session store/event schema, protocol
  DTOs, provider auth/readiness, and TUI stream/state management.
- Writer agents must work from the same baseline docs: this file, the
  developer guidelines, the source-of-truth matrix, the active plan, and
  the relevant subsystem docs.
- Explorer and verifier agents may inspect broadly, but writer agents
  should have disjoint write sets. If write sets overlap, coordinate
  through the integrator before editing.
- When a writer subagent returns, the integrator must review the actual
  diff before marking the task done. A subagent summary describes
  intent, not outcome.

## Code Quality

- `#![forbid(unsafe_code)]` is the default rule. Do not introduce `unsafe` without a documented reason.
- Keep provider support checks, model support checks, and auth capability checks centralized.
- Keep TUI keybindings configurable. Do not hardcode terminal key matches inline when a binding table exists or should exist.
- Ask before removing intentional user-facing behavior or narrowing support.

## Verification

After code changes, run the smallest set of checks that honestly covers the touched path:

```bash
cargo fmt --all
cargo check -p <affected-crate>
cargo test -p <affected-crate>
```

For cross-cutting changes, expand to adjacent crates or:

```bash
cargo check --workspace
cargo test --workspace
```

If you create or modify tests, you must run the affected test-bearing crate until it passes.

When testing the launcher from a workspace checkout, prefer:

```bash
cargo run -p belltower --
```

That path uses fresh workspace helpers.
Do not assume previously built `target/debug/bt-server` or `target/debug/bt-tui` binaries are current unless you rebuilt them intentionally.

## TUI Testing

Use the documented `tmux` recipe in [`docs/development/tui-testing.md`](./docs/development/tui-testing.md) for repeatable full-screen testing.

## Provider Work

When adding or changing a provider, follow [`docs/development/provider-integration-checklist.md`](./docs/development/provider-integration-checklist.md).

## Auth Work

When adding auth sources, imports, or subscription-backed flows, follow [`docs/development/auth-migration-and-imports.md`](./docs/development/auth-migration-and-imports.md).

## Git Safety

- Stage only files changed in the current slice.
- Never use `git add .` or `git add -A`.
- Always inspect `git status` before committing.
- Never use destructive cleanup commands in a shared dirty worktree.
- If a conflict appears in a file outside the current slice, stop and ask.

## Commit Discipline

- Do not commit unless the task explicitly asks for commits or the user requests them.
- When committing, use explicit `git add <path>` entries for only the files you changed.
