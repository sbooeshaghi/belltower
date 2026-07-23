# Belltower Target Implementation Goals

This document turns the current Belltower design into an implementation contract.
It is based on:

- Belltower's v0.3 design in [`../architecture/overview.md`](../architecture/overview.md)
- local review of `pi` and `pi-mono`
- local review of `hermes-agent`
- the rationale in Mario Zechner's "What I learned building an opinionated and minimal coding agent"

The purpose is to define:

- the exact product we are trying to ship
- the functional requirements that must be satisfied
- the infrastructure and engineering expectations that must hold
- the human-run milestone tests we can use while building

This is not a wishlist. It is the target bar.
Near-term sequencing lives in [`./platform-roadmap.md`](./platform-roadmap.md); this document defines the stable requirements that sequencing must respect.

## 1. Product Goal

Belltower must become a fully functioning standalone Rust agent harness for AI scientists, with first-class OpenInference and OpenTelemetry access from the harness itself, not bolted on around someone else's runtime.

The target end state is:

1. a scientist can install Belltower and start it with a single `belltower` command
2. they can authenticate a supported cloud provider or use a local model backend
3. they can work through a full TUI and server/client workflow with tools, approvals, branching, steering, and MCP
4. every meaningful step is captured in a canonical Belltower-owned session/event store aligned with OpenInference semantics
5. the same session can be replayed, exported, and emitted to OTLP/OpenInference backends without losing fidelity

The reason to build Belltower instead of adopting an existing harness is not novelty. It is ownership of the runtime, ownership of the telemetry, and ownership of a Rust-native implementation path suitable for scientific and research workflows.

## 2. Design Principles We Are Carrying Forward

From Belltower's design:

- session-as-telemetry is the canonical model
- the event taxonomy and write path are part of the product boundary
- server-first transport is mandatory
- raw stream durability is mandatory
- projection-backed reads are mandatory
- OpenInference alignment is core, not optional
- local API security is mandatory
- data-driven provider/model catalogs are mandatory

From `pi`:

- the harness should stay predictable and inspectable
- the session format and context transitions must be explicit
- branching and compaction are first-class, not hacks
- the TUI must surface model/tool/session state clearly
- minimality is good when it improves observability and control

From Hermes:

- onboarding must be coherent
- `setup`, `login`, `model`, `doctor`, and `status` must tell the same story
- provider/runtime resolution must be centralized
- MCP and tool runtime must be first-class subsystems
- operator workflows matter as much as agent-loop internals

## 3. Release Target

We should treat the project as three layers.

### 3.1 Core Stable

This is the first release bar that must feel like a real harness:

- protocol
- client/server
- session storage and replay
- runtime composition
- pure agent loop
- supported providers
- core tools
- approvals
- cancel and steer
- branching
- compaction
- MCP
- auth and onboarding
- TUI
- OpenInference-aligned event model

### 3.2 Standalone Complete

This is the bar where Belltower feels like a mature standalone product:

- local model discovery and lifecycle
- export surfaces
- OTLP/OpenInference export
- richer TUI operations and settings flows
- dataset-ready export surfaces over multiple sessions, branches, and workflow lineage

### 3.3 Experimental Later

This stays feature-gated and outside the initial compatibility promise:

- self-improvement
- isolated worktree proposal flows
- generalized multi-agent scheduling, recursive delegation policy, and role systems
- plugin hosting beyond MCP/custom tools
- trainer- and RL-specific integrations on top of canonical trajectory datasets

Belltower now has bounded child-session execution with explicit lineage,
per-child provider/model settings, and typed durable parent-child messages.
Autonomous model-facing spawn is approval-gated; manual spawn remains an
explicit authenticated operator action that does not start child execution.
This is not a general workflow scheduler. Delegated coding at scale still
requires isolated worktrees, and broader recursion or role policy must land in
a separate composition layer only when real workloads justify it.

User-owned trajectory datasets are also part of the long-term design, but not the current release bar.
When they land, they should be derived from the canonical session/event store as stable artifacts for replay, evaluation, and training rather than introducing a second runtime path.

## 4. Exact Functional Requirements

The following requirements are the implementation target.

### FR-1. Single User-Facing Entry Point

- The primary user command is `belltower`.
- `belltower` must handle first-run setup, local server startup, and TUI launch.
- `bt-server` and `bt-tui` may exist as helper binaries, but the product entry point is `belltower`.
- `belltower setup`, `belltower login`, `belltower logout`, `belltower model`, `belltower doctor`, and `belltower status` must all work coherently.

### FR-2. Truthful Provider And Model Support

- Belltower must only present runnable providers as runnable.
- Unsupported or planned providers must be explicitly marked as planned and must not be offered as default runnable paths.
- Auth onboarding must not imply that incompatible external tokens are valid API credentials.
- Supported providers must resolve through one provider/runtime registry, not ad hoc checks spread across the codebase.

### FR-3. First-Class Auth System

- Credentials must live in a Belltower-owned auth store.
- The auth store must be versioned and cross-process safe.
- API-key providers must support:
  - literal secret storage
  - environment variable references
  - shell-command references
- `doctor` and `status` must report where credentials are resolved from.
- Local server auth must use a generated bearer token for all non-health routes.
- Subscription-backed auth, when added later, must be modeled as separate provider/auth paths, not overloaded into generic API-key providers.

### FR-4. Canonical Session/Event Store

- The canonical record must be a Belltower-owned SQLite store.
- The store must contain:
  - `sessions`
  - `branches`
  - `events`
  - `raw_chunks`
  - projection tables for messages, approvals, tool runs, branch heads, and cost summaries
- Every event must have a monotonic sequence identifier.
- Replay must work by `Last-Event-ID` without scanning serialized enum blobs.

### FR-5. OpenInference-First Event Model

- Every relevant agent/runtime event must carry the fields required for OI-compatible tracing.
- OpenInference alignment is structural, not nominal.
- The canonical event schema should keep Belltower-owned field names such as provider/model/token/tool fields and map them to evolving OI attribute names in `bt-otel`.
- Required semantic coverage includes:
  - session/turn/span identity plus turn-derived exported trace identity
  - turn identity and explicit turn lifecycle boundaries
  - agent, chain, LLM, and tool span kinds
  - provider/model identifiers
  - token usage
  - cost
  - tool metadata
  - structured inputs and outputs
- OTLP export must derive from the canonical session store, not bypass it.
- Runtime `tracing` instrumentation must mirror committed canonical events rather than becoming a second source of truth.
- The canonical event taxonomy must be stable enough to serve as the real coordination surface across storage, SSE, export, and TUI rendering.

### FR-6. Raw Stream Durability

- Provider output must be persisted as both:
  - canonical structured events
  - raw provider chunks
- Raw chunks must be queryable and exportable.
- Raw chunk storage must support, at minimum:
  - querying all chunks for a turn on a branch
  - querying all chunks for a particular LLM call within a session
  - comparing structured projections against raw provider data for the same turn
- Raw chunk schema/index design must preserve the identifiers needed for those access patterns.
- Stream parsing improvements must not require rerunning old sessions to reconstruct provider behavior.

### FR-7. Server-First Protocol

- `bt-protocol` must be the single canonical wire contract.
- `bt-server` must expose the control plane over HTTP/SSE.
- `bt-client` must be the only Rust client interface.
- `bt-tui` must always talk through `bt-client`.
- SSE streams must support reconnect and replay through `Last-Event-ID`.
- Protocol version negotiation and typed error envelopes must be part of the supported surface, not an implementation detail.

### FR-8. Pure Agent Loop

- `bt-agent` must remain storage-agnostic and server-agnostic.
- `bt-agent` must not depend on `bt-session`, `bt-runtime`, `bt-server`, or `bt-auth`.
- During active development, the stronger practical rule is that `bt-agent` should only close over `bt-core` and `bt-tools` among workspace crates, and that boundary should be enforced mechanically.
- It must handle:
  - prompt/request execution
  - streaming accumulation
  - tool-call dispatch
  - tool-result re-entry
  - retries
  - finish-state handling
  - steer/cancel consumption
  - per-turn budget/completion semantics inside the pure loop
- The runtime-facing turn boundary must stay typed and explicit.
- The agent emits turn outcomes and structured turn data for the runtime to persist; it does not write directly to the canonical store.

### FR-9. Instructions And Skills

- Belltower v1 must include a minimal markdown-based instruction system.
- Instruction resolution must support:
  - managed built-in harness prompt assets
  - global skill directory
  - project skill directory
  - deterministic composition order
  - on-demand loading
- The active instruction set used for a turn must be reconstructable from stored session state.

### FR-10. Core Tooling And Guardrails

- Built-in tools must include:
  - `read`
  - `write`
  - `edit`
  - `list`
  - `search`
  - `shell`
  - `web_search`
  - `web_fetch`
  - `plan`
  - `ask`
  - `inspect`
- Later built-ins such as `branch`, `delegate`, and `undo` should remain in the architectural plan, but they should not block the near-term normalization of the core built-in set.
- Cross-session memory should grow through the explicit `memory.*` subsystem surface rather than assuming a separate foundational `recall` built-in.
- The model-facing built-in vocabulary should converge on short canonical names and a unified `inspect` tool rather than a proliferation of verbose `*_file` and `inspect_*` names.
- Operational commands that do not justify a native tool may still run through `shell` and `bt ...`.
- Tool guardrails must include:
  - project-root enforcement
  - `.gitignore` awareness where appropriate
  - binary detection
  - truncation
  - per-file mutation serialization
  - timeout and cancel behavior
  - process-tree cleanup
- Tool results must distinguish model-visible output from richer UI/export details where useful.

### FR-11. Human Approval Flow

- Approval state must be first-class and persisted.
- The runtime must support:
  - policy-based auto-approval
  - explicit human approve
  - explicit human deny
  - pending state with turn suspension
  - replay of approval events
- Approval pause/resume must remain an explicit runtime boundary rather than hidden blocking IO inside the agent loop.
- The TUI must make approvals obvious and easy to resolve.

### FR-12. Cancel, Steer, And Autonomous Control

- Cancellation must be a first-class runtime control, not a best-effort afterthought.
- Steering must be first-class in v1 and must be persisted as part of the session history.
- Autonomous runs must support explicit budgets and completion checks.
- Long-running sessions must remain interruptible and redirectable from the TUI and protocol surface.

### FR-13. Branching, Tree Navigation, And Compaction

- Sessions must support first-class branching.
- Users must be able to switch branches and resume from any branch head.
- Branch summaries must preserve context when switching away from active work.
- Compaction must preserve enough state to keep working reliably.
- Compaction ownership should live in `bt-runtime`, not `bt-agent`.
- The runtime should compact before handing an over-budget history to the agent loop rather than making the agent discover context-window failure mid-turn.
- Compaction correctness must guarantee:
  - tool calls and tool results are not separated
  - file-read/file-modified metadata survives
  - summaries preserve actionable next-step context
  - compacted context fits the target budget
- Compaction events must record what was dropped or summarized and the before/after accounting that produced the compacted state.

### FR-13a. Session Lineage And Multi-Session Inspectability

- Belltower must treat session lineage as a first-class inspectability concern for manual and model-facing child-session execution.
- The canonical session model must be able to represent:
  - parent/child session relationships
  - delegation or spawn origin
  - related turn or branch origin where applicable
- The server/client/TUI inspection surfaces must be able to answer:
  - which related sessions belong to the same higher-level workflow
  - what happened in each related session
  - how branch history and session lineage differ
- This requirement keeps bounded delegation reproducible and auditable and
  prevents future multi-agent workflows from becoming opaque side channels.

### FR-13b. Delegated Session Semantics

- Belltower models subagent execution as child sessions, not as ordinary branches.
- Child sessions must have:
  - explicit parent session, branch, and turn origin
  - their own queue, approvals, cancel, and steer state
  - their own canonical session log and their own turn-scoped exported traces
- Parent-child messages must use typed durable mailbox events with explicit
  delivery and resolution state, not transcript reconstruction or hidden
  process-local state.
- Until isolated worktrees land, shared-project-root execution must be explicit
  and described truthfully. Model-facing autonomous spawn must be
  approval-gated; manual spawn is an authenticated operator action that creates
  but does not execute a child. Delegated coding at scale should use isolated
  worktrees by default.
- Higher-order scheduling, role systems, and recursive delegation policy are
  outside the per-session runtime contract.

### FR-14. First-Class MCP

- MCP must be a first-class runtime subsystem, not a bolt-on.
- Belltower must support:
  - stdio MCP servers
  - streamable HTTP/SSE MCP servers
  - discovery
  - runtime lifecycle management
  - reconnect/degraded status
  - dynamic tool registration into the normal tool path
- MCP tools must go through the same approval, session, and telemetry pipeline as built-in tools.

### FR-15. Local Models

- Belltower must support local OpenAI-compatible backends.
- The local-model subsystem must detect and report:
  - Ollama
  - LM Studio
  - llama.cpp-compatible servers
- Standalone completeness should also include model lifecycle actions:
  - search
  - pull/import
  - remove
  - load/unload where supported
  - recommendations based on hardware profile

### FR-16. Strong Operator UX

- The TUI must be excellent, not merely usable.
- It must support:
  - session creation and resume
  - session list/tree visibility
  - streaming chat
  - multiline composition
  - tool activity visibility
  - approval actions
  - branch switching
  - session refresh
  - provider/model/MCP status visibility
  - export access
- The interactive experience should also support the operator patterns that clearly work in Hermes and `pi`:
  - coherent setup/login/model/doctor/status flows
  - discoverable commands or shortcuts
  - visible streaming progress
  - visible session and branch context
- CLI/TUI behavior must feel coherent and discoverable, following the good patterns from Hermes and `pi`.

### FR-17. Export And Analysis

- Belltower must export sessions as:
  - JSONL
  - HTML
  - ShareGPT-style format
- It must also export OI/OTel-compatible traces through `bt-otel`.
- A user must be able to inspect raw chunks, structured events, and exported traces for the same session.

### FR-18. Protocol Compatibility And Versioning

- The first stable protocol must come with an explicit compatibility promise.
- Protocol evolution must be guarded by golden tests and backward-compatibility checks.
- Reconnect behavior, replay behavior, and typed error behavior must remain stable across compatible minor releases.

### FR-19. App And Repo Independence

- No Belltower crate may depend on Frolllo internals.
- No Belltower crate may depend on app-specific schemas or services.
- Future integrations consume Belltower only through:
  - `bt-protocol`
  - `bt-client`
  - MCP/custom tool boundaries

## 5. Infrastructure And Engineering Expectations

### IR-1. Rust Workspace Discipline

- The Belltower workspace must remain cleanly split by responsibility.
- `#![forbid(unsafe_code)]` remains the default.
- shared dependencies are declared at the workspace level
- crate boundaries are real, not nominal

### IR-2. Single Source Of Truth

- There must be one canonical session store.
- There must be one canonical protocol.
- There must be one canonical runtime/provider resolution path.
- There must not be a hidden "special TUI path" that bypasses server/client behavior.
- There must not be a hidden "tracing-only" or "UI-only" path that creates canonical-looking events without a prior store commit.

### IR-3. Storage Semantics

- SQLite runs in WAL mode.
- Session/event writes go through a single-writer path.
- Read performance comes from maintained projections rather than scanning serialized payloads.
- Raw stream durability is not optional and cannot be disabled in the stable bar.
- Canonical events are only broadcast or mirrored after SQLite commit succeeds and `seq_id` is assigned.
- SSE/UI delivery must be driven by the committed event, not by the tracing mirror.
- Concurrent sessions may interleave through the same canonical writer path, but global ordering and per-session identifiers must remain stable and queryable.

### IR-4. Deterministic Config Layering

- Built-in catalog/data files are the base.
- Global config overlays built-ins.
- Project config overlays global config.
- Auth state stays separate from non-secret config.

### IR-5. Security By Default

- local API is bearer-token protected
- credential files use owner-only permissions where supported
- tool/runtime guardrails are on by default
- destructive behavior requires an explicit approval path

### IR-6. Observability By Construction

- internal session data must be enough to reconstruct and analyze behavior
- OTLP export is downstream of stored data
- provider/tool/runtime events must be inspectable without patching the harness
- runtime instrumentation spans may exist without canonical events, but canonical session events must never bypass the store

### IR-7. Data-Driven Catalogs

- providers, pricing, context windows, and model recommendations must remain data-driven where possible
- changing the provider catalog should not require deep code edits

### IR-8. Honest Capability Reporting

- setup, doctor, status, and model selection must report support truthfully
- no feature should be presented as working unless it is implemented and testable

### IR-9. Testability

- subsystems must expose seams for fixture-based and end-to-end tests
- provider parsing and protocol behavior must be golden-testable
- storage and replay must be deterministic under test
- approval pause/resume and turn outcomes must be testable without requiring a live server

### IR-10. CI Expectations

- `cargo fmt --all`
- `cargo check --workspace`
- targeted crate tests
- workspace tests where toolchain stability allows
- future CI should include representative end-to-end harness tests, not just unit tests
- CI should eventually verify crate-boundary purity for `bt-agent` so storage/server/auth dependencies cannot leak in accidentally

### IR-11. Error And Failure Taxonomy

- The architecture should distinguish at least:
  - provider errors
  - tool errors
  - protocol/reconnect errors
  - storage/runtime errors
- Error rendering and retry policy should be able to depend on that distinction.

### IR-12. Cost Accounting Semantics

- Cost projection must be based on token usage plus the Belltower pricing catalog in effect when the relevant event is committed.
- Historical session costs must remain stable enough to keep `/usage` trustworthy.
- Later pricing-catalog changes must not silently rewrite old session cost surfaces in place.

### IR-13. Failure Semantics

- If the canonical SQLite store is unavailable because of migration, disk, or locking failure, canonical turn progression should stop.
- Belltower should not silently fall back to a hidden non-durable mode for ordinary interactive use.

### IR-14. Documentation Expectations

- the design doc stays architecture-focused
- this target doc stays requirement-focused
- quickstart and operator docs must reflect actual runnable flows
- unsupported providers/features must be documented honestly

### IR-15. Dogfooding Requirement

- Belltower must be able to work effectively on its own repo before we call the harness stable
- that includes auth, sessions, tools, branching, compaction, and telemetry/export inspection

## 6. Human Milestone Tests

These are the manual tests a human should be able to run as we progress.

### M0. Install And Health

Goal: the product starts and reports its state coherently.

- Run `belltower setup`.
  - Expected: supported providers and local backends are presented truthfully, credentials can be stored coherently, and a default connection/model is chosen without editing files by hand.
- Run `belltower doctor`
  - Expected: config path, auth store path, server helper, TUI helper, and connection status are shown clearly.
- Run `belltower status`
  - Expected: supported vs planned providers are explicit.
- Run `belltower`
  - Expected: if setup is missing, the launcher walks through setup instead of failing obscurely.

### M1. Local Model Happy Path

Goal: local development works with no cloud dependency.

- Start a local backend such as Ollama.
- Run `belltower setup` and choose `local`.
- Run `belltower`.
  - Expected: server launches, TUI opens, first session can be created.
- Ask a simple question.
  - Expected: assistant response streams and session data is persisted.

### M2. Cloud Provider Happy Path

Goal: a supported cloud provider works end to end.

- Run `belltower login openai` or `belltower login anthropic`.
- Run `belltower model` and select the matching provider/model.
- Run `belltower`.
- Ask a simple question.
  - Expected: response streams, costs/tokens are tracked, and `status` reports the credential source truthfully.

### M2.1. Control Plane And Protocol Health

Goal: the protocol surface is a real supported interface.

- Start `belltower`, then inspect the local server health route.
  - Expected: `GET /health` succeeds without auth and reports healthy state.
- Use a valid bearer token to query session or stream routes.
  - Expected: non-health routes require auth and typed protocol errors are returned on bad requests.
- Reconnect to an SSE session stream with `Last-Event-ID`.
  - Expected: replay behavior is stable and ordered.

### M3. Tool Use And Approvals

Goal: the agent can act and the user can control it.

- Ask the agent to read a file in the repo.
  - Expected: tool call is visible in the TUI and the result is stored.
- Ask the agent to run a shell command that requires approval.
  - Expected: approval prompt appears and the turn pauses.
- Approve once, then deny on a second run.
  - Expected: both approval events are persisted and replayable.

### M4. Session Replay And Raw Chunks

Goal: the harness is inspectable.

- Create a session with at least one streamed response and one tool call.
- Use server/client/TUI inspection commands to view session messages and raw chunks.
  - Expected: raw provider payloads and structured events both exist.
- Reconnect to the SSE stream using `Last-Event-ID`.
  - Expected: replay resumes cleanly.

### M4.1. Long Session Resume

Goal: the harness remains responsive on sessions with very large histories.

- Create or script a session with thousands of messages/tool events/raw chunks.
- Resume the session from the TUI.
  - Expected: the TUI loads a latest transcript window quickly instead of replaying the entire session.
- Scroll upward near the top of the transcript.
  - Expected: older history is backfilled lazily without blocking the live operator loop.

### M5. Branching And Tree Continuity

Goal: history can fork without losing context.

- Create a session, then branch from an earlier point.
- Continue the new branch.
- Switch back to the original branch.
  - Expected: branch heads are distinct and recoverable.
- Trigger a branch summary when leaving work.
  - Expected: summary survives reload and helps continuation.

### M6. Compaction Correctness

Goal: long-running work can continue after context compression.

- Create or script a session long enough to force compaction.
- Continue working after compaction.
  - Expected: the agent still remembers active work and relevant files.
- Inspect the stored compaction metadata.
  - Expected: file tracking and cut-point behavior are correct.

### M7. MCP Integration

Goal: external tool servers behave like native parts of the harness.

- Configure a simple stdio MCP server.
- Verify its tools appear in Belltower.
- Call an MCP tool from a session.
  - Expected: tool call, approval handling, runtime state, and telemetry behave like built-in tools.
- Repeat with streamable HTTP/SSE MCP once implemented.

### M7.1. Instructions And Skills

Goal: the instruction system is real and inspectable.

- Add a project skill in `.belltower/skills`.
- Start a session that should load it.
  - Expected: the active instruction composition is deterministic and the session behavior reflects the loaded skill.
- Repeat with a global skill and confirm project-level composition remains predictable.

### M8. Telemetry Export

Goal: the project's core reason for existing is satisfied.

- Run a representative session containing LLM calls, tool calls, approvals, and branching.
- Export JSONL and HTML.
- Export OTLP/OpenInference spans through `bt-otel`.
- Inspect the result in Phoenix or another OTLP consumer.
  - Expected: spans/events match the session history and contain model/tool/token/cost metadata.

### M9. Dogfooding On The Belltower Repo

Goal: Belltower can develop itself.

- Use Belltower on the Belltower repository for several real tasks:
  - read and edit files
  - run tests
  - branch and switch
  - compact long sessions
  - inspect/export telemetry
- Expected: the harness is stable enough that we would choose it over ad hoc shell + external agent tooling for day-to-day work on its own codebase.

## 7. Non-Goals For The Initial Stable Bar

These are intentionally not required for the first stable bar:

- generalized multi-agent scheduling, role systems, and isolated-worktree lifecycle
- generalized plugin hosting beyond MCP/custom tools
- self-improvement against the active worktree
- pretending ChatGPT/Codex subscription auth is the same as OpenAI API access
- Frolllo-specific integration behavior inside Belltower crates

Notably, bounded child-session execution does not make Belltower a general
multi-agent framework. Session lineage, durable mailbox state, and
cross-session inspection remain part of the stable architectural bar; broader
workflow policy remains experimental.

## 8. Definition Of Done

Belltower hits the target implementation goal when:

1. all core stable requirements are implemented
2. the milestone tests through M8 are passing in practice
3. the harness can dogfood itself on the Belltower repo
4. OI/OTel export is accurate enough to inspect real agent work as first-class telemetry
5. setup, auth, sessions, tools, MCP, and TUI all feel coherent enough that a human would actually choose to use the harness
6. long-running sessions remain inspectable and resumable without forcing the UI to reconstruct entire histories synchronously
