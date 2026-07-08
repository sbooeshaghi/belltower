# Belltower Subsystem Parity Matrix

This document grounds Belltower's implementation in concrete subsystems.
It exists to answer four questions:

1. what subsystems Belltower actually has
2. which parts are core to the product versus explicitly out of scope
3. what we should copy or adapt from `pi` and Hermes
4. how a human can test each subsystem as we build it

It should be used together with:

- [`../architecture/overview.md`](../architecture/overview.md) for architectural decisions
- [`./target-implementation-goals.md`](./target-implementation-goals.md) for requirements and milestone tests

This is not a package list for marketing. It is the working map for implementation.

## 1. Product Boundary

Belltower is not trying to clone the entire `pi` or Hermes ecosystem.

Belltower's core product is:

- a standalone Rust agent harness
- with a canonical Rust session/event store
- a canonical HTTP/SSE protocol boundary
- a first-class TUI and launcher
- first-class tool, MCP, and provider orchestration
- first-class OpenInference and OpenTelemetry access from the harness itself

That means some things in `pi` or Hermes are relevant as subsystem patterns, while others are not part of Belltower's first target.

Examples of things we should learn from but not necessarily reproduce:

- `pi-mom` style chat platform delegation
- Hermes gateway and messaging platform adapters
- `pi-pods` style GPU deployment tooling
- a standalone web component library

Examples of things we should copy the spirit of aggressively:

- `pi`'s session, tree, compaction, export, and tool-guardrail behavior
- Hermes's setup/login/model/doctor/status coherence
- Hermes's operator-facing TUI expectations
- both tools' emphasis on explicit inspectability

## 2. Package-Style System Table

This table is intentionally modeled after the `pi` package list format, but adapted to what Belltower actually needs.

| Belltower crate / surface | Description |
| --- | --- |
| `belltower` | User-facing launcher and operator CLI. Handles first-run setup, login, model selection, doctor, status, local server startup, and TUI launch. |
| `bt-core` | Shared types, IDs, errors, config, traits, and OpenInference-aligned event envelope primitives. |
| `bt-protocol` | Versioned HTTP/SSE contract: DTOs, typed errors, SSE envelopes, replay semantics, version negotiation. |
| `bt-client` | Canonical Rust client for `bt-protocol`. Every Rust consumer, including the TUI, uses this path. |
| `bt-session` | SQLite storage, migrations, append-only event log, raw chunk durability, replay, branches, and projection-backed reads. |
| `bt-runtime` | Composition layer for sessions, approvals, control queues, connections, instructions, telemetry, and runtime services. |
| `bt-agent` | Pure turn loop: streaming, tool-call dispatch, tool-result re-entry, retries, finish handling, steer/cancel/autonomous control. |
| `bt-providers` | Provider adapters and transport logic for OpenAI-compatible APIs, Anthropic, and later other providers. |
| `bt-tools` | Built-in tool implementations and guardrails for files, shell, search, and URL reads. |
| `bt-context` | Token accounting, compaction, summarization, file tracking, branch summaries, and context assembly. |
| `bt-auth` | Credential store, auth resolution, auth imports, refreshable auth later, and local server token management. |
| `bt-mcp` | MCP discovery, stdio/HTTP transports, lifecycle management, degraded-state handling, and tool registration. |
| `bt-server` | Canonical `axum` server exposing Belltower over HTTP/SSE. |
| `bt-tui` | Full-screen interactive terminal client for sessions, branches, approvals, tools, provider status, exports, and MCP state. |
| `bt-models` | Local model discovery, recommendations, and later model lifecycle actions like pull/import/remove/load/unload. |
| `bt-otel` | Export from the canonical session store into JSONL, HTML, ShareGPT, and OTLP/OpenInference traces. |
| `bt-improve` | Experimental self-improvement subsystem, isolated worktrees only, outside the first compatibility promise. |

## 3. Case-Study Mapping

This table maps the major Belltower surfaces to the closest useful case-study analogue.

| Belltower | `pi` analogue | Hermes analogue | How to use the analogy |
| --- | --- | --- | --- |
| `belltower` | `@mariozechner/pi-coding-agent` CLI surface | `hermes`, `hermes setup`, `hermes model`, `hermes doctor` | Copy the coherent first-run/operator UX, not the whole runtime architecture. |
| `bt-core` + `bt-agent` | `@mariozechner/pi-agent-core` | Hermes agent core and loop internals | Copy behavioral invariants for turns, tools, and state transitions. |
| `bt-providers` | `@mariozechner/pi-ai` | Hermes provider/model resolution | Copy provider abstraction patterns, not TypeScript implementation details. |
| `bt-tui` | `@mariozechner/pi-tui` plus `pi` interactive mode | Hermes full TUI | Copy operator ergonomics and visibility patterns. |
| `bt-session` + `bt-context` | `pi` session, tree, and compaction subsystems | Hermes session storage and compaction behavior | Copy explicit history/tree/compaction semantics. |
| `bt-auth` | `pi` auth store and OAuth registry | Hermes login/model/config auth flows | Copy auth UX and storage discipline. |
| `bt-mcp` | `pi` MCP and external tool wiring | Hermes MCP integration | Copy first-class treatment of MCP as part of the runtime. |
| `bt-otel` | `pi` export subsystem | Hermes logs/trace-oriented observability ideas | Copy inspectability and export affordances, but keep Belltower's OI/OTel-first center. |

## 4. Subsystem Matrix

This is the implementation table we should work from.

| Subsystem | Primary crates | Why it exists | Required behavior | `pi` / Hermes lessons | Human test |
| --- | --- | --- | --- | --- | --- |
| Launcher and onboarding | `belltower` | Give the user one coherent entrypoint | `belltower` launches the product, `setup`/`login`/`model`/`doctor`/`status` tell the same story, local server can be started automatically | Copy Hermes's coherent onboarding flow; do not scatter auth/model/server logic | Run `belltower doctor`, `belltower status --probe`, `belltower setup`, then `belltower` |
| Auth and credential resolution | `bt-auth`, `belltower` | Make cloud/local access explicit and inspectable | Versioned auth store, cross-process safety, env/command/literal support, truthful credential source reporting, later imports from external tools | Copy `pi` auth store discipline and Hermes's auth UX; keep subscription auth separate from API key auth | Run `belltower login <provider>`, inspect `doctor`/`status`, verify no hidden auth assumptions |
| Provider/runtime registry | `bt-core`, `bt-providers`, `bt-runtime` | Keep provider support centralized and truthful | One registry of runnable vs planned providers, one path for validation and completion execution | Copy Hermes's provider resolution coherence; avoid ad hoc provider checks | `status --probe` should match actual runtime support |
| Protocol boundary | `bt-protocol`, `bt-client`, `bt-server` | Make the harness consumable and replayable | Versioned DTOs, typed errors, SSE envelopes, reconnect, `Last-Event-ID`, stable compatibility guarantees | Hermes reinforces API surface discipline; Belltower's protocol must remain canonical | Call health, protected routes, and SSE replay from a real client |
| Session/event store | `bt-session` | Own the runtime history and telemetry | SQLite, append-only events, raw chunks, branches, projections, replay without scanning enum blobs | Copy `pi`'s explicit session management and Hermes's persistence expectations | Create a session, stream, reconnect, inspect events and projections |
| OpenInference event model | `bt-core`, `bt-session`, `bt-runtime` | Make telemetry first-class rather than post hoc | OI-compatible span semantics for agent, chain, LLM, tools, token usage, cost, IO | This is the key Belltower-specific differentiator; do not compromise it for parity | Inspect stored events and verify they can reconstruct traces |
| Raw stream durability | `bt-providers`, `bt-session`, `bt-runtime` | Preserve exact provider behavior for replay and export | Persist both structured events and raw provider bytes/chunks | `pi`'s inspectability points in this direction; Belltower must go further | Stream a response, inspect raw chunks and structured events side by side |
| Agent turn loop | `bt-agent` | Keep orchestration pure and reusable | Streaming, tool dispatch, tool-result re-entry, retries, finish states, steer/cancel consumption, per-turn budget/completion semantics | Copy behavior, not architecture, from both tools | Run a tool-using turn and verify suspension, re-entry, and completion |
| Tools and guardrails | `bt-tools`, `bt-runtime` | Let the agent act without destroying the workspace | Root enforcement, `.gitignore`, truncation, binary detection, process cleanup, mutation serialization | Copy `pi`'s file mutation queue and guardrail strictness | Read, edit, and shell-run with approvals; verify no races or orphaned processes |
| Approval system | `bt-runtime`, `bt-server`, `bt-tui` | Keep humans in the control loop | Persisted pending/approved/denied state, policy auto-approval, pause/resume behavior | Hermes and `pi` both show that the UI matters as much as the runtime | Trigger a shell approval, approve once, deny once, replay both |
| Instructions and skills | `bt-runtime`, `bt-context` | Make harness behavior composable and inspectable | Built-in + global + project markdown skills, deterministic composition, reconstructable active instructions | Hermes's skill discipline is useful, but Belltower should stay minimal in v1 | Add a project skill and verify it is loaded deterministically |
| Branching and tree navigation | `bt-session`, `bt-runtime`, `bt-context`, `bt-tui` | Make long-running work forkable and inspectable | Explicit branches, branch heads, branch summaries, branch-aware reconstruction | Copy `pi`'s tree/session model directly in spirit | Branch from an earlier turn, continue both branches, switch and resume |
| Compaction and summarization | `bt-context`, `bt-session` | Keep long sessions viable | Token budgeting, tool/result pairing, file tracking, branch summaries, actionable summaries | Copy `pi` compaction invariants and Hermes's visibility around compaction pressure | Force compaction and verify the agent can continue coherent work |
| MCP runtime | `bt-mcp`, `bt-runtime`, `bt-server`, `bt-tui` | Make external tools first-class | stdio + streamable HTTP/SSE, degraded status, reconnect, dynamic tool registration, same approval/telemetry path as built-ins | Copy both tools' first-class MCP stance | Add an MCP server, inspect its status, call a tool, replay the result |
| Local model subsystem | `bt-models`, `belltower`, `bt-providers` | Support local inference honestly | Detect backends, tie configured connection to actual backend, expose recommendations, later add lifecycle actions | Hermes and `pi` both teach truthful capability reporting; no fake local readiness | Run `doctor`, `status --probe`, and `setup --connection local`; verify model/backend truthfulness |
| TUI / operator interface | `bt-tui`, `bt-client` | Make the harness operationally pleasant | Full-screen chat, multiline input, tool activity, approvals, branches, session navigation, provider/model/MCP state, exports | Copy `pi` and Hermes operator ergonomics aggressively | Launch `belltower`, create or resume sessions, approve tools, switch branches |
| Export and analysis | `bt-otel`, `bt-session`, `bt-server`, `bt-tui` | Let users inspect sessions from multiple angles | JSONL, HTML, ShareGPT, OTLP/OpenInference export from the canonical store | Copy `pi`'s export affordances, but keep Belltower's telemetry store canonical | Export one session in multiple formats and compare them |
| Server runtime | `bt-server`, `bt-runtime` | Give every client the same control plane | Sessions, messages, approvals, replay, MCP, models, exports, health, auth, SSE | Hermes reinforces that API/server surfaces become permanent quickly | Use HTTP/SSE as the source of truth, not hidden local shortcuts |
| Experimental self-improvement | `bt-improve` | Explore reviewable self-improvement safely | Isolated worktrees only, proposal ledger, reviewable apply/dismiss/revert flows | Hermes is useful as inspiration, but Belltower keeps this outside the first stable bar | Not required for initial stable testing |

## 5. How The Subsystems Fit Together

The main runtime path should stay simple:

1. `belltower` resolves config, auth, and local server state
2. `bt-tui` uses `bt-client` against `bt-server`
3. `bt-server` delegates to `bt-runtime`
4. `bt-runtime` coordinates `bt-session`, `bt-agent`, `bt-context`, `bt-tools`, `bt-providers`, and `bt-mcp`
5. all meaningful behavior is recorded in `bt-session`
6. `bt-otel` and other exports read from stored session data, not from side channels

That implies a few invariants:

- the TUI must never bypass the protocol
- the provider layer must never bypass session telemetry
- OTLP export must never bypass the session store
- MCP tools must look like normal tools to the rest of the runtime
- compaction and branching must operate on canonical session state, not ad hoc TUI memory

## 6. What We Should Copy Directly

### Copy Directly In Spirit From `pi`

- explicit branch/tree semantics
- compaction invariants and tests
- file mutation serialization
- export usability
- inspectable session transitions

### Copy Directly In Spirit From Hermes

- coherent first-run setup UX
- login/model/doctor/status consistency
- operator-facing TUI expectations
- centralized provider/runtime resolution
- MCP treated as a core subsystem, not an extension afterthought

## 7. What We Are Explicitly Not Porting Whole

We should not attempt a wholesale port of `pi` or Hermes into Rust.

Reasons:

- Belltower's architecture is centered on a canonical Rust session store and OI/OTel event model
- a full port would still require architectural reinterpretation, not just translation
- the goal is to copy mature subsystem behavior, not inherit a mismatched runtime core

So the rule should be:

- port subsystem behavior
- port invariants and tests
- port UX patterns
- do not port architecture blindly

## 8. Recommended Implementation Order From This Matrix

Given the current Belltower state, the next slices should be:

1. Finish release closure for the hardening stack
   Reason: the next leverage point is making the release gates, tests, and checked-in planning surface match the code that now exists.

2. Protocol compatibility discipline and consumer artifacts
   Reason: the server-first contract now needs stronger schema discipline, typed consumer surfaces, and golden coverage.

3. Explicit host/bootstrap separation and reusable server composition
   Reason: remote clients and local hosts should sit on distinct seams without weakening `bt-protocol` as the public boundary.

4. Live OTLP validation and dataset/replay foundation
   Reason: Belltower's telemetry moat should be proven against real consumers, then extended into replay and dataset artifacts.

5. Structured return artifacts and worktree policy for delegated sessions
   Reason: workflow execution should build on stable inspection, export, and dataset foundations rather than arriving first.

## 9. Human Test Checklist By Subsystem

Use this as the running manual checklist while implementing.

| Subsystem | Manual checkpoint |
| --- | --- |
| Launcher | `belltower`, `belltower setup`, `belltower doctor`, `belltower status --probe` |
| Auth | `belltower login`, `belltower logout`, credential-source visibility in `doctor` and `status` |
| Local models | `belltower setup --connection local`, then `doctor` to verify actual backend/model truthfulness |
| Protocol | health route, protected route auth, SSE replay with `Last-Event-ID` |
| Session store | create a session, reconnect, inspect messages, events, and raw chunks |
| Tools | read/edit/shell with visible results and approvals |
| Approvals | pending, approve, deny, replay |
| Branching | create branch, switch branch, resume branch, inspect branch summaries |
| Compaction | force compaction and verify continuity |
| MCP | configure server, see degraded/ready state, call tool, replay tool event |
| TUI | create/resume sessions, multiline input, tool visibility, approvals, branches |
| Exports | JSONL, HTML, ShareGPT, OTLP/OpenInference |

## 10. Definition Of Success For This Document

This document is useful only if it helps us implement Belltower subsystem by subsystem without losing sight of the whole product.

If a proposed feature or refactor does not clearly land in this matrix, we should ask:

- is it required for the stable harness?
- is it optional standalone polish?
- is it experimental later?
- or is it outside the product boundary entirely?
