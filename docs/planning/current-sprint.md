# Current Sprint: Release Hardening And Audit Closure

This document defines the current focused sprint for Belltower.

Read it together with:

- [`../architecture/overview.md`](../architecture/overview.md)
- [`../architecture/event-taxonomy-and-write-path.md`](../architecture/event-taxonomy-and-write-path.md)
- [`../architecture/provider-output-and-transcript-model.md`](../architecture/provider-output-and-transcript-model.md)
- [`../architecture/tool-system-and-normalization.md`](../architecture/tool-system-and-normalization.md)
- [`../development/source-of-truth-matrix.md`](../development/source-of-truth-matrix.md)
- [`./target-implementation-goals.md`](./target-implementation-goals.md)
- [`./platform-roadmap.md`](./platform-roadmap.md)
- [`./harness-landscape-review-20260408.md`](./harness-landscape-review-20260408.md)
- [`./pre-improvement-subsystem-review-20260408.md`](./pre-improvement-subsystem-review-20260408.md)

The current stack is the release-hardening stack, not the earlier coherence
pass.

The major delivered slices are:

- `codex/p1-safety`
  - filesystem containment, auth fallback, literal-secret handling, and shell
    auto-approval fixes
- `codex/p2-control-plane`
  - durable approval scopes, canonical queue/control inspection, resumed-turn
    ordering, and settings-revision semantics
- `codex/p3-tui-launcher`
  - TUI and launcher adoption of canonical queue and readiness state
- `codex/p4-telemetry-export`
  - resumed-turn export ordering fixes and transitional JSON bundle export
- `codex/p5-provider-auth-readiness`
  - structured readiness truth and cleaner client-consumer boundary guidance
- `codex/p6-mcp`
  - explicit MCP lifecycle states and coherent inventory reporting
- `codex/p7-docs-parity`
  - not-found API contract cleanup and final docs-planning alignment

That changed the sprint.
The job now is to finish the audit cleanly enough that the release can merge
without hidden contradictions.

## Sprint Theme

Close the release-hardening stack by making the active docs, planning surface,
tests, and pre-merge review gates match the implemented system.

This sprint is about trust, release readiness, and preserving the architectural
decisions we have already made.

## Why This Sprint Now

The hardening stack already addressed the highest-risk seam problems:

- control-plane durability and ownership
- paused and queued settings semantics
- TUI control-state ownership
- export and resumed-turn provenance
- readiness truth and client-consumer boundaries
- MCP lifecycle visibility
- not-found protocol behavior

What remains is smaller but still release-critical:

- historical audit inputs must be checked in and classified correctly
- planning docs must stop recommending completed work
- final verification must run on the full stacked branch
- final review must happen against `main`, the architecture docs, and the
  developer guidelines before merge

If this is skipped, the code may be correct while the repo still teaches future
contributors the wrong shape.

## Sprint Goals

### 1. Make the checked-in planning surface truthful

Target outcomes:

- the audit documents used to drive the hardening plan live in the repo
- those dated audits are clearly classified as reference or historical input
- `current-sprint`, `platform-roadmap`, and `subsystem-parity-matrix` reflect
  the post-hardening state rather than the pre-hardening queue

### 2. Finish the release gate

Target outcomes:

- final workspace verification is run against the stacked branch
- release-facing subsystem docs do not materially overclaim the implementation
- any remaining must-fix review findings are resolved before merge

### 3. Preserve the architecture for post-release work

Target outcomes:

- release closure does not reopen settled control-plane seams
- future work is sequenced on top of the hardened boundaries
- usability work stays downstream of the canonical runtime and protocol path

## Exact Development Focus

Implementation order for this sprint:

1. check in the dated audit inputs and classify them correctly in the docs
2. rewrite active planning docs so they describe the current release state
3. run final workspace verification on the stacked branch
4. run a final multi-agent review against `main`, the architecture docs, and
   the developer guidelines
5. merge only after the release blockers list is empty

## Implementation Checklist

- [x] Land the safety and correctness fixes from the earlier audit
- [x] Canonicalize control-plane ownership, queue inspection, and settings
  revisions
- [x] Move TUI and launcher control surfaces onto canonical inspection and
  readiness state
- [x] Fix resumed-turn export ordering and bundle fidelity gaps
- [x] Tighten readiness truth and remote-consumer boundary docs
- [x] Clarify MCP lifecycle and inventory surfaces
- [x] Fix the `404 not_found` contract for missing release-facing resources
- [x] Check in the dated audit inputs used by the hardening plan
- [x] Reclassify the audit inputs in the source-of-truth docs
- [x] Run `cargo fmt --all`
- [x] Run `cargo check --workspace`
- [x] Run `cargo test --workspace`
- [ ] Run the documented TUI and MCP smoke/acceptance checks for release
- [x] Spawn the final pre-merge review against `main` and the developer
  guidelines

## Non-Goals For This Sprint

Not in scope unless a release blocker makes them unavoidable:

- new delegated-session or autonomous-subagent behavior
- broad new provider families
- deeper local-model lifecycle actions
- dataset/replay feature expansion
- server-lib or host-bootstrap refactors that do not block the release

## Design Rules For This Sprint

Every change in this sprint should preserve these rules:

1. the canonical store remains the source of truth
2. the runtime and store own reusable control-plane state
3. the server stays a transport and orchestration surface, not a shadow runtime
4. the TUI and launcher stay consumers of canonical state
5. planning docs describe current release gates and future sequencing, not
   historical queues

## Human Checkpoints

### Release doc truth

1. read `current-sprint`, `platform-roadmap`, and `subsystem-parity-matrix`
2. compare them with the stacked branch history and current code
3. verify that they describe what remains rather than what is already done

Expected:

- planning docs point to the next real work, not the previous hardening work

### Release contract truth

1. read the protocol, readiness, MCP, and launcher/TUI subsystem docs
2. compare them with the current `bt-server`, `bt-client`, `bt-readiness`,
   `bt-mcp`, `belltower`, and `bt-tui` surfaces
3. verify that release-facing docs do not overclaim durability or availability

Expected:

- operator and client docs describe the implemented release surface accurately

### Final merge gate

1. run the workspace verification suite
2. perform the final multi-agent review against `main` and the developer
   guidelines
3. merge only after the remaining blocker list is empty

Expected:

- the release branch is technically green and narratively truthful
