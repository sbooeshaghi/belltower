# Pre-Friend-Testing Audit — 2026-04-01

This document records the highest-value changes Belltower should make before it is handed to friends for real dogfooding.

It is informed by:

- direct review of the current Belltower code and docs
- `cargo check --workspace`
- recent TUI and provider debugging work
- local review of the reference harnesses in `/tmp`:
  - Codex
  - Claude Code
  - Hermes
  - pi-mono

This is not a redesign-from-scratch note.
It is a practical pre-dogfooding punch list.

## Current Strengths

Belltower is already strong in the places that matter most for a serious harness:

- one canonical event/session store
- one server-first protocol boundary
- one trace per turn for OpenInference export
- durable raw chunk capture
- explicit session, branch, lineage, queue, and execution inspection
- a managed harness prompt stack instead of ad hoc server-local strings
- a normalized built-in tool vocabulary with approval metadata

Those are real advantages over lighter harnesses.
The goal now is to make the product layer as coherent as the architecture.

## What The Reference Harnesses Suggest

### Codex

Copy:

- strong Rust crate boundaries
- inline terminal model as the default interaction path
- explicit protocol/server separation
- clear distinction between canonical logic and UI projection

Do not copy:

- shell-first tool semantics as Belltower's canonical telemetry vocabulary

### Claude Code

Copy:

- subsystem separation
- clear plugin/skill/task/memory layering
- operator polish around streaming, approvals, and slash-command ergonomics

Do not copy:

- a huge monolithic query/UI core as the long-term shape for Belltower

### Hermes

Copy:

- self-improvement as an explicit loop over skills and memory, not an invisible mutation path
- coherent onboarding and operator command discoverability
- the idea that the harness should inspect and learn from prior sessions

Do not copy:

- making the learning loop outrun the canonical runtime model

### pi-mono

Copy:

- session/tree semantics
- message queueing and branch-aware continuation
- customization surfaces for skills/extensions/themes without collapsing the core harness

Do not copy:

- JSONL/session-file storage as the primary Belltower substrate

## Blocking Changes Before Friend Testing

### 1. Stabilize the inline TUI and remove the remaining scroll-era leftovers

This is the biggest product risk right now.

Evidence:

- recent regressions have all clustered in `bt-tui`
- `cargo check --workspace` currently reports dead code only in `bt-tui`
- [main.rs](/Users/sinabooeshaghi/projects/frollo/belltower/crates/bt-tui/src/main.rs) is about 12k lines and is carrying both the new inline model and remnants of the old app-scrolled model

Concrete changes:

- remove the dead scroll-era state and helpers in `bt-tui`
- split [main.rs](/Users/sinabooeshaghi/projects/frollo/belltower/crates/bt-tui/src/main.rs) into focused modules:
  - transcript model
  - terminal substrate
  - input/editor
  - slash commands
  - approval/pending panels
  - session loading and stream reconciliation
- add a small tmux/iTerm/Terminal.app smoke matrix that is run intentionally after any TUI-affecting slice
- keep the transcript immutable once emitted; prefer inspection and drill-down over replay-driven mutation
- reduce the persistent footer command clutter and move most operator help into startup scrollback and `/help`
- add one-time tmux modified-key guidance so multiline/key-chord failures are easier to diagnose

Why this blocks sharing:

- a harness that duplicates messages, clips inspect output, or corrupts layout under streaming will lose trust immediately

### 2. Make auth tokens server-instance-scoped instead of globally shared

This is a real correctness bug, not polish.

Evidence:

- launching a second local `bt-server` can invalidate an already-running client by rotating the shared bearer token
- the current launcher/server flow still uses a mutable global token file
- the failure mode already surfaced in live use as `401 Unauthorized`

Concrete changes:

- make the bearer token belong to the specific `bt-server` instance a client is talking to
- stop rereading a globally mutable token in long-lived clients
- store or pass server-instance auth material together with the selected server endpoint

Why this blocks sharing:

- opening a second session or smoke test should never silently break another running session

### 3. Finish the provider compatibility harness for real models

The recent `content: null` bugs are a warning.

Evidence:

- both local OpenAI-compatible models and hosted OpenAI models exposed serializer edge cases
- the current OpenAI-compatible adapter contains a growing set of subtle message-shape rules in [openai.rs](/Users/sinabooeshaghi/projects/frollo/belltower/crates/bt-providers/src/openai.rs)

Concrete changes:

- add explicit provider-wire tests for:
  - empty assistant content
  - tool-call-only assistant messages
  - reasoning-only assistant messages
  - tool-result-followup turns
  - error-tool-result recovery turns
- run a short real-provider matrix before external dogfooding:
  - hosted OpenAI
  - at least one strict OpenAI-compatible local backend
  - Anthropic

Why this blocks sharing:

- “works on one model but fails mysteriously on another” is one of the fastest ways to lose trust in a harness

### 4. Decide what is actually in scope for the first friend test

Belltower currently exposes a lot of surface area.

Evidence:

- there is already a large command/protocol/inspection surface
- `bt-improve` exists in the workspace but is only a bootstrap stub in [lib.rs](/Users/sinabooeshaghi/projects/frollo/belltower/crates/bt-improve/src/lib.rs)

Concrete changes:

- define the first friend-test surface explicitly:
  - launcher/setup/login/model/status
  - TUI chat/session/resume/branching
  - core tools
  - inspect/errors/tool inspection
  - export/telemetry only if the setup is ready
- keep clearly experimental subsystems hidden or visibly marked experimental

Why this blocks sharing:

- friends should be testing the product you intend to stand behind, not every partially-shaped subsystem in the repo

## High-Value Changes Soon After Friend Testing Starts

### 5. Pull runtime-backed tools out of `bt-server` ownership

Today `plan`, `ask`, and `inspect` are canonical Belltower tools, but their implementations still live under `bt-server` in:

- [session_tools.rs](/Users/sinabooeshaghi/projects/frollo/belltower/crates/bt-server/src/session_tools.rs)
- [inspection_tools.rs](/Users/sinabooeshaghi/projects/frollo/belltower/crates/bt-server/src/inspection_tools.rs)

That is understandable, but it is not the clean end state.

Recommended direction:

- keep environment tools in `bt-tools`
- move runtime-backed canonical tools toward `bt-tools` too
- inject narrow runtime-facing traits instead of passing server/runtime ownership downward

Why this matters:

- server assembly and canonical tool ownership should not keep drifting together

### 6. Break up the giant files before they become architectural prisons

Current file sizes:

- [bt-tui/src/main.rs](/Users/sinabooeshaghi/projects/frollo/belltower/crates/bt-tui/src/main.rs): about 12k lines
- [bt-server/src/main.rs](/Users/sinabooeshaghi/projects/frollo/belltower/crates/bt-server/src/main.rs): about 6.3k lines
- [bt-runtime/src/runtime.rs](/Users/sinabooeshaghi/projects/frollo/belltower/crates/bt-runtime/src/runtime.rs): about 3.5k lines

Codex and Claude Code both show the value of module-level decomposition even when the overall product is large.

Recommended direction:

- split `bt-tui` first
- split `bt-server` routes, registry assembly, session actions, export, and TUI/launcher-facing helpers next
- only split `bt-runtime` after its public seams are clearer

Why this matters:

- right now too much correctness depends on not touching the wrong region of one giant file

### 7. Keep inspection as the universal drill-down path

This is one of the best recent Belltower decisions.

Keep moving in this direction:

- compact immutable transcript summaries in chat
- drill-down through `inspect`
- the same inspection model for humans and agents

Recommended follow-ups:

- richer `inspect tool <call-id>` output formatting for humans
- stable inspect coverage for errors, raw chunks, and future memory lookups
- small helper affordances in transcript lines so call ids are easy to use

### 8. Add a real memory seam before shipping memory itself

Memory should be the next big differentiator, but not as an ad hoc prompt hack.

Given the Semaphora direction, the right long-term shape is:

- canonical Belltower traces/events remain the source of truth
- memory is a derived semantic graph over those traces
- memory references are inspectable and attributable back to session, turn, span, tool call, or branch

Recommended near-term work:

- define a memory interface in architecture docs before implementation
- make memory retrieval a runtime-backed tool or inspect surface, not a hidden prompt side channel
- preserve provenance all the way from memory node back to the originating Belltower event sequence

Why this matters:

- Belltower should become the harness whose memory is explainable, not just persistent

### 9. Build a stronger web-research layer

The initial `fetch` tool proved that URL retrieval belongs in the
canonical tool path, but it was too small for scientific web research.
Belltower now treats web retrieval as a first-class two-tool seam:
`web_search` for source discovery and `web_fetch` for specific public
URL retrieval.

Recommended direction:

- backend-selectable web search providers
- clear fetch/extract/read separation
- better traceability of web-derived evidence
- eventual integration with the same inspection and memory model

### 10. Keep the event/trace/session model disciplined

This is mostly in good shape now.
Do not regress it.

Keep enforcing:

- session is the long-lived container
- turn is the trace boundary
- `llm_call` and `tool_execution` are spans inside the turn
- canonical events are committed before SSE/tracing/export

This remains one of Belltower's strongest architectural choices.

## Code Hygiene Findings

As of this audit:

- `cargo check --workspace` succeeds
- the only workspace warnings are in `bt-tui`, and they are dead-code leftovers from the old scroll-era implementation
- the docs tree still contains repo cruft like `.DS_Store`, which should be removed

The important rule for the next phase should be:

- no dead-code warnings in the main workspace
- no unexplained giant-file growth
- no new public surface without a matching subsystem or architecture note

## Recommended Order

If the goal is “share with friends soon, but keep architectural quality high,” the order should be:

1. stabilize and modularize the inline TUI enough to stop layout/regression churn
2. fix server-instance auth scoping
3. run the provider compatibility matrix on real backends
4. explicitly trim or label the first dogfood surface
5. remove dead code and easy repo hygiene issues
6. then move into tool/module cleanup, memory seams, and stronger web research

## Bottom Line

Belltower is already unusually strong where most harnesses are weak:

- canonical state
- inspectability
- telemetry
- branching and workflow lineage

Before friend testing, the main job is not inventing more architecture.
It is making the product layer match the architecture:

- stable TUI
- stable auth
- stable provider behavior
- stable expectations about what is ready to be used
