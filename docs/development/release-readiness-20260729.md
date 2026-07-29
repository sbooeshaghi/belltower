# Release Readiness Review — 2026-07-29

## Decision

**NO-GO for tagging `0.1.0` yet.** The code-critical release defects found in
this review are closed. The remaining bar is immutable release evidence: run
the complete matrix and manual lanes from the clean commit that will receive
the tag, including the live OTLP consumer and M9 dogfood requirements below.

This decision uses the canonical event log as the product boundary: a feature
is not release-complete merely because a control request is accepted. Its
observable execution and terminal state must also be durably truthful.

## Release blockers

### 1. Resolved: cancellation interrupts exact live work truthfully

Cancellation now targets the exact active `(session, branch, turn)`. The
canonical `session.cancelled` event commits before the in-memory signal is
delivered, and cancellation wins terminal-state races without permitting a
later assistant result to contradict the log. Provider construction and
streams, compaction summarization, immediate and deferred tools, and
approval-resumed work observe the same capability.

Shell cancellation and timeout terminate and reap the complete process tree
through POSIX process groups or a Windows Job Object; capture is bounded while
the pipes are drained, and agent and operator paths honor the validated
configured timeout. MCP
`WaitForCompletion` calls are bounded, and an unknown timed-out external
operation resets the transport and degrades readiness instead of inventing a
result. Every requested tool records exactly one terminal result before the
turn records `cancelled` and releases active ownership.

Deterministic tests cover held providers, simultaneous compaction completion,
cancel-versus-provider-error and tool-admission races, blocking filesystem
search, shell process trees, approval-resumed shell work, MCP timeout/reset/
degradation, and cancel-versus-natural-finish races. The real tmux acceptance
also holds a mock provider turn open, cancels it, exports the session, and
asserts exactly one cancelled `TurnFinished` with no later same-turn assistant
message.

### 2. Resolved: 0.1 uses source/archive distribution

`0.1.0` deliberately does not publish workspace crates to crates.io. This
avoids unrelated registry packages already named `belltower` and `bt-runtime`
without introducing an under-reviewed public-package rename. Every workspace
crate sets `publish = false`; the supported paths are verified GitHub release
archives and the three-binary `cargo install --path` source-checkout flow.

`scripts/distribution.sh --preflight` checks that contract and
`scripts/distribution.sh --install-check` proves the local install shape and
launcher helper discovery. A future registry release is a new product/API
decision: reserve and verify every public crate name, define its compatibility
policy, then restore publication intentionally rather than treating it as a
release-time fallback.

### 3. Resolved: `web_fetch` pins validated DNS answers per hop

Each request and redirect now resolves once, rejects empty or mixed answer
sets and every non-globally-routable special-use address, normalizes
IPv4-mapped and well-known NAT64-embedded IPv4, and pins the complete validated
public answer set into the HTTP connector. The original URL remains intact so
HTTP Host and TLS SNI stay correct, and proxies are disabled for this direct
fetch path so they cannot reintroduce an independent resolution step. One
total deadline bounds DNS, headers, redirects, and body consumption.

Adversarial tests cover special-use IPv4/IPv6, mapped and NAT64 private
targets, mixed answers, DNS rebinding, private redirects, stalled DNS and
headers, and dripping bodies, and prove that private listeners are never
contacted.

### 4. Required immutable operational evidence is incomplete

The following diagnostic acceptance passed on the integrated worktree on
2026-07-29:

- isolated source-checkout installation and `doctor` helper discovery for
  `belltower`, `bt-server`, and `bt-tui`
- a live Ollama-backed local session with exact-token output
- a live authenticated ChatGPT-backed cloud session with exact-token output
- the real tmux operator flow with three normal turns, cancellation, steering,
  shell approval/resume, inspection, compaction, and JSONL export; its exported
  canonical log passed the cancellation terminal-event assertions, and the
  harness exited without leaving a tmux session, child process, or temp root
- a real OpenTelemetry Collector Contrib consumer ingested and independently
  exposed two traces and eight correlated turn/model/tool spans from the
  canonical-store-derived protobuf; see
  [`otlp-consumer-acceptance-log-20260729.md`](./otlp-consumer-acceptance-log-20260729.md)
- Belltower's TUI performed a real Day 1 task on this repo with heterogeneous
  local/cloud children, sibling and parent messaging, branch switching,
  compaction, inspection, cancellation, a targeted test, and JSONL/OTLP
  export; see [`dogfood-log-20260729.md`](./dogfood-log-20260729.md)
- the full deterministic workspace, strict Clippy, architecture-boundary,
  file-size, OpenAPI, acceptance, and source-distribution gates

These runs validate behavior, but the worktree is intentionally uncommitted
during the review, so they are not the final exact-SHA release record.

The final release commit still needs dated, exact-SHA evidence for:

- one supported local provider and one supported cloud provider
- the tmux operator flow, including real cancel and steer behavior
- representative OI/OTel ingestion in Phoenix or another OTLP consumer
- M9 Belltower-on-Belltower dogfooding, including tools, branching,
  compaction, subagents with heterogeneous models, related-session messaging,
  inspection, and export
- the successful manually dispatched, non-publishing release-candidate matrix
  on all five supported release targets

The default acceptance runner intentionally skips these credentialed or
interactive lanes. Deterministic proxies do not satisfy the manual release bar.

## Mechanical defects closed in this review

- exact-turn cancellation now interrupts all release-critical work classes and
  preserves a single truthful canonical terminal history
- `web_fetch` pins validated public DNS answers for each redirect hop and fails
  closed on rebinding, special-use/NAT64 targets, and stalled responses
- shell timeout/cancellation uses process-tree containment on Unix and Windows,
  bounds captured output, and has executable Windows CI/release tests
- tmux acceptance teardown removes its exact session, child processes, and
  temporary state on both success and failure
- source/archive distribution is explicit: every workspace crate is private to
  the repository, and CI verifies the supported local install shape
- Windows sibling/PATH helper discovery applies the platform executable suffix
- the release tag must equal the workspace package version
- build/check/test commands in CI and release verification use `--locked`
- the retired `macos-13` x86_64 runner was replaced
- GitHub Release publication occurs once, after all five platform artifacts
  exist, rather than independently from each matrix job
- release archives contain `LICENSE`; the aggregate release contains
  `SHA256SUMS`
- native matrix jobs and the local install check verify that `belltower doctor`
  resolves both helper binaries
- a yanked transitive TUI dependency was pinned back to a non-yanked version
- auth tokens are created owner-only and atomically replace existing files on
  Unix, so permissive or partially written token contents are never exposed
- the non-publishing release-candidate workflow runs deterministic acceptance,
  package preflight, local-install validation, and all five target builds
- manual release-candidate jobs have read-only repository permission; only a
  semver tag-push publication job receives `contents: write`
- explicitly requested tmux acceptance fails instead of silently skipping when
  `tmux` is unavailable
- the `0.1.x` protocol compatibility policy is documented as preview behavior

## Final immutable GO checklist

Run all commands on the clean commit that will receive the tag, using Rust
1.93.0 and its checked-in lockfile:

```bash
git status --porcelain
git diff --check
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p bt-agent --test purity --locked
bash scripts/check_crate_boundaries.sh
bash scripts/file_size_lint.sh
make openapi-check
make acceptance
bash scripts/distribution.sh --preflight
bash scripts/distribution.sh --install-check
```

Once this workflow is present on the repository default branch, make the
candidate SHA the unchanged tip of the intended release ref and manually
dispatch the release workflow for that ref. Verify that the workflow run SHA
equals the candidate SHA. This runs the same verification and all five platform
builds, but its publication job is restricted both by event/ref conditions and
by a tag-only write token. Record the live local, live cloud, tmux,
OTLP-consumer, and dogfood evidence above on the same SHA. Only after those
checks are complete should `v0.1.0` be pushed at that unchanged SHA. The
tag-triggered release workflow must then finish verification, all five builds,
the aggregate artifact-count check, and the single GitHub-release publication
job.

## Explicitly deferred, non-blocking work

- isolated worktrees and generalized recursive scheduling
- delta-efficient continuous remote export (correct append-only/CAS behavior
  exists; transport efficiency remains an acknowledged gap)
- signatures, SBOMs, and provenance beyond the new checksum manifest
- generated SDKs and package managers beyond the selected `0.1.0`
  distribution path
