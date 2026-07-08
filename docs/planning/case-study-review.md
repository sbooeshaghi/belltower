# Belltower Case Study Review

This document records what Belltower should learn from three mature agent toolchains:

- `pi-mono`
- `hermes-agent`
- OpenAI Codex (`codex-rs`)

It exists to answer a practical question:

- which behaviors, subsystem boundaries, and operator UX patterns should Belltower copy directly
- which should be adapted to Belltower's Rust-native, telemetry-first architecture
- which should be rejected because they conflict with Belltower's product goal

This document should be read together with:

- [`../architecture/overview.md`](../architecture/overview.md)
- [`./target-implementation-goals.md`](./target-implementation-goals.md)
- [`./subsystem-parity-matrix.md`](./subsystem-parity-matrix.md)

It is not a design replacement.
It is a case-study-based implementation guide.

## 1. Sources Reviewed

The comparison in this document is based on local review of:

### `pi-mono`

- `/tmp/belltower-case-studies/pi-mono/AGENTS.md`
- `/tmp/belltower-case-studies/pi-mono/packages/coding-agent/README.md`
- `/tmp/belltower-case-studies/pi-mono/packages/agent/README.md`
- `/tmp/belltower-case-studies/pi-mono/packages/tui/src/autocomplete.ts`

### Hermes

- `/tmp/belltower-case-studies/hermes-agent/website/docs/user-guide/cli.md`
- other adjacent Hermes docs already used in the current Belltower planning work

### Codex

- `/tmp/belltower-case-studies/codex/README.md`
- `/tmp/belltower-case-studies/codex/codex-rs/README.md`
- `/tmp/belltower-case-studies/codex/codex-rs/core/README.md`
- `/tmp/belltower-case-studies/codex/codex-rs/protocol/README.md`
- `/tmp/belltower-case-studies/codex/codex-rs/app-server/README.md`
- `/tmp/belltower-case-studies/codex/codex-rs/login/src/auth/storage.rs`
- `/tmp/belltower-case-studies/codex/codex-rs/otel/README.md`

## 2. Overall Conclusion

The three tools are useful for different reasons:

- `pi` is the best donor for session/tree semantics, message queuing, compaction discipline, and practical TUI interaction patterns.
- Hermes is the best donor for coherent onboarding, login/model/status flows, and operator-facing CLI/TUI ergonomics.
- Codex is the best donor for Rust crate boundaries, app-server protocol shape, auth storage modes, sandbox policy visibility, and explicit telemetry/export separation.

Belltower should not port any of them wholesale.
The right strategy is:

- copy subsystem behavior
- copy lifecycle invariants
- copy operator UX patterns
- keep Belltower's own architecture:
  - canonical SQLite session/event store
  - canonical HTTP/SSE protocol boundary
  - OpenInference-first event model
  - OpenTelemetry export derived from stored session data

## 3. System Comparison Table

| System | What it is best at | What Belltower should take |
| --- | --- | --- |
| `pi-mono` | Mature coding-agent workflow, session tree model, command UX, message queuing, compaction behavior | Session branching semantics, queue/interrupt behavior, autocomplete patterns, export usability, explicit tool and session inspectability |
| Hermes | Productized onboarding and operator ergonomics | `setup`/`login`/`model`/`doctor`/`status` coherence, truthful provider readiness, strong CLI/TUI interaction patterns, slash-command discoverability |
| Codex (`codex-rs`) | Rust-native workspace design, app-server protocol, auth storage modes, sandbox framing | Crate-boundary discipline, thread/turn/item lifecycle modeling, auth-store design, protocol/server documentation style, first-class sandbox and telemetry boundaries |

## 4. Subsystem Lessons

## 4.1 Launcher, Setup, Login, Model, Doctor, Status

### Hermes

Hermes has the strongest operator story:

- one obvious entry command
- coherent setup/login/model/status flows
- clear distinction between interactive and non-interactive modes
- resume options that are explicit, not magical

### Codex

Codex reinforces:

- one default CLI entrypoint
- one workspace with explicit subcommands
- install, config, and runtime surfaces documented from the top

### Belltower implication

Belltower should keep:

- `belltower` as the only primary entrypoint
- `belltower setup`
- `belltower login`
- `belltower model`
- `belltower doctor`
- `belltower status`
- `belltower resume`

And Belltower should continue making those commands tell one consistent story about:

- configured default connection/model
- actual runtime readiness
- credential source
- local backend truthfulness

## 4.2 TUI Layout And Input Model

### `pi`

`pi`'s interactive mode has a strong model:

- fixed editor/input area
- fixed footer/status area
- slash commands integrated into the editor rather than as a separate mode
- message queue semantics that are first-class
- session tree and session resume are part of the interaction model, not hidden features

### Hermes

Hermes adds:

- persistent status bar above the input
- slash-command autocomplete dropdown
- clear resume display
- explicit interrupt behavior
- streaming tool output in the main interface

### Codex

Codex is a reminder that:

- the TUI should be one client over a protocol/runtime boundary
- the same core should support TUI and non-interactive execution

### Belltower implication

Belltower should keep moving toward:

- stable layout with reserved space for interactive affordances
- first-class slash commands and completions
- no transcript jump when menus appear
- fixed input semantics with real editing support
- event-driven transcript updates
- a status model that is useful but not visually noisy

Specific patterns worth copying:

- reserved area under the input for slash suggestions and approval choices
- fixed-status/operator surface that does not shove messages around
- session resume as an explicit action rather than implicit reuse
- TUI-only state driven from protocol events, not local guesses

## 4.3 Slash Commands And Command Registry

### `pi`

`pi`'s command model is strong because:

- commands are first-class
- command completion is handled by a central autocomplete provider
- command arguments can have their own completions
- commands unify built-ins, skills, and extensions

### Hermes

Hermes reinforces:

- `/` should immediately open a discoverable command list
- commands should be case-insensitive and obvious
- command help belongs in the interaction flow, not hidden in docs alone

### Belltower implication

Belltower should adopt:

- a central command registry
- one source of truth for:
  - help text
  - completion
  - argument completion
  - execution
- reserved command UI space under the input
- keyboard navigation of command suggestions

This should become the basis for later additions like:

- `/resume`
- `/tree`
- `/tools`
- `/usage`
- `/export`
- `/mcp`
- `/settings`

## 4.4 Session, Resume, Branching, And Tree Navigation

### `pi`

This is one of `pi`'s strongest areas:

- sessions are explicit and inspectable
- branching is first-class
- `/tree` and `/fork` are real tools for long-running work
- compaction is understood as lossy context compression on top of a preserved full history

### Hermes

Hermes has a clear resume story:

- continue latest
- resume specific session
- recap on resume

### Codex

Codex app-server formalizes this well:

- `thread/start`
- `thread/resume`
- `thread/fork`
- explicit thread and turn lifecycle events

### Belltower implication

Belltower should align these ideas into its own model:

- default `belltower` starts a new session
- `belltower resume` resumes explicitly
- branching stays a core Belltower feature
- branch summaries and branch-aware reconstruction remain part of the stable bar
- the event model should continue to treat session/branch transitions as first-class events

The Codex thread/turn/item model is especially useful as a conceptual cross-check:

- Belltower sessions roughly correspond to threads
- Belltower agent turns should stay explicit in the event log
- Belltower messages, tool calls, approvals, and raw chunks should remain item-like units with stable ordering

## 4.5 Agent Loop, Event Flow, And Tool Calling

### `pi`

`pi-agent-core` is very clear about:

- turn boundaries
- message start/update/end
- tool execution start/update/end
- tool-result re-entry creating another model turn
- `continue()` semantics

### Hermes

Hermes's agent-loop docs reinforce:

- approvals are part of the turn lifecycle
- interrupts and redirects must be represented clearly
- streaming and tool execution must feed the interface as events, not as inferred state

### Codex

Codex app-server doubles down on:

- explicit lifecycle events
- connection initialization
- start/resume/fork semantics
- notification-driven rich clients

### Belltower implication

Belltower should keep the following invariants strict:

- approvals are gates within one continuous turn loop
- tool results must preserve the authoritative tool-call ID
- the follow-up model turn must always happen against updated history
- TUI pending approval state must be driven by events, not transcript heuristics
- tool execution, model streaming, and follow-up turns must all be replayable from stored events

This is not optional.
It is one of the core structural reasons Belltower exists.

## 4.6 Message Queueing And Interrupts

### `pi`

`pi` has the clearest message queue model reviewed so far:

- `Enter` queues steering
- `Alt+Enter` queues follow-up
- queued messages can be restored
- interrupt behavior is explicit
- queue delivery mode is configurable

### Hermes

Hermes reinforces the user expectation:

- new input during work should interrupt or redirect meaningfully
- in-progress tools/commands need real cancellation behavior

### Belltower implication

Belltower should implement queueing as a first-class subsystem, not a TUI hack.

Target behavior:

- input while the model is working should not be lost
- steering vs follow-up intent should be explicit
- queue state should be visible in the UI
- interrupt/cancel semantics should be coherent across:
  - model calls
  - shell tools
  - approvals
  - queued user messages

This should be one of the next major TUI/runtime slices.

## 4.7 Auth Storage And Credential Management

### Hermes

Hermes shows the UX side:

- users expect `login` to be a first-class command
- auth state should be coherent with `model`, `doctor`, and `status`

### `pi`

`pi` shows:

- auth is its own subsystem
- auth storage is not just environment variables
- imports and multi-provider auth flows matter

### Codex

Codex adds the most concrete Rust-native pattern:

- explicit auth storage modes:
  - file
  - keyring
  - auto
  - ephemeral
- `auth.json` as a structured store
- `0600` file permissions
- keyring fallback and migration behavior

### Belltower implication

Belltower should evolve toward:

- a versioned Belltower-owned `auth.json`
- explicit storage mode support
- likely:
  - `file`
  - `keyring`
  - `auto`
  - `ephemeral`
- truthful credential-source reporting in `doctor` and `status`
- provider-auth resolution that stays centralized

Important limit:

- subscription-backed auth may be added later
- it must be modeled as separate provider/auth paths
- it must not be overloaded into generic API-key providers

## 4.8 Protocol And App-Server Shape

### Codex

Codex is the strongest case study here.
Its app-server reinforces several useful patterns:

- protocol is explicit and documented
- initialization is a formal handshake
- rich clients subscribe to lifecycle notifications
- thread/turn/item modeling clarifies responsibility
- generated schemas are a supported part of the developer experience
- protected routes/transports and readiness endpoints are explicit

### Hermes

Hermes is less protocol-centric in the docs reviewed, but it reinforces that once external clients exist, the runtime surface becomes part of the product.

### Belltower implication

Belltower should continue strengthening:

- `bt-protocol` as the canonical contract
- HTTP/SSE as the current supported surface
- event-driven clients through `bt-client`
- typed errors and replay guarantees

And Belltower should consider future improvements inspired by Codex:

- more formal client initialization/handshake semantics
- clearer thread/session lifecycle naming in the protocol
- better schema generation and protocol docs
- richer event subscriptions or filtering options later

## 4.9 Sandbox, Approval, And Guardrails

### Codex

Codex is the clearest case study for making sandbox policy first-class:

- explicit sandbox modes
- dedicated flags for selecting them
- docs that treat sandboxing as part of the product, not a hidden implementation detail

### `pi` and Hermes

Both reinforce that approval UX is part of usability, not just safety.

### Belltower implication

Belltower should continue to make these first-class:

- approval scopes
- runtime tool guardrails
- shell process cleanup
- mutation serialization
- truthful tool pending/approved/executing state

And Belltower should likely grow a more explicit sandbox/approval policy surface in the launcher and docs.

## 4.10 MCP And Extensibility

### Codex

Codex treats MCP as a core part of the CLI and app-server story.

### Hermes

Hermes reinforces MCP as a first-class runtime subsystem.

### `pi`

`pi` is a useful reminder that extensions, skills, and tool ecosystems become part of daily usability when they are discoverable and operationally coherent.

### Belltower implication

Belltower should continue treating MCP as core stable behavior:

- visible in `doctor` and `status`
- visible in the TUI
- same approval and telemetry path as built-in tools
- degraded-state handling must be honest and inspectable

## 4.11 Telemetry And Exports

### `pi`

`pi` is strongest on session export usability.

### Codex

Codex is strongest on crate boundary separation:

- core/runtime logic
- app-server protocol
- OTEL integration crate

### Belltower implication

Belltower should keep:

- canonical event/session store first
- `bt-otel` as export logic derived from that store
- JSONL, HTML, ShareGPT, and OTLP/OpenInference as inspection surfaces

This is the center of gravity for the project.
Belltower is not building observability as an afterthought.
It is building a harness whose execution is inspectable as telemetry.

## 5. Copy / Adapt / Reject

## 5.1 Copy Directly In Spirit

- `pi` message queue semantics
- `pi` session tree and fork semantics
- `pi` slash-command completion structure
- `pi` compaction invariants and tests
- Hermes operator command coherence
- Hermes status-bar and slash-command discoverability
- Codex Rust crate-boundary discipline
- Codex app-server lifecycle explicitness
- Codex auth storage mode design
- Codex sandbox policy visibility

## 5.2 Adapt To Belltower

- `pi` JSONL session ideas should become SQLite event-store semantics in Belltower
- Hermes resume model should map onto Belltower sessions and branches, not replace them
- Codex thread/turn/item concepts should inform Belltower protocol naming and event design, not force a transport rewrite
- Codex auth model should inform storage and lifecycle, not current provider support claims
- `pi` extension richness should be adapted through MCP and skills, not by rebuilding its plugin architecture wholesale

## 5.3 Reject

- wholesale porting of `pi`, Hermes, or Codex architecture
- hiding provider/runtime truth behind optimistic UI
- bypassing the canonical session store for telemetry/export
- treating TUI-local state as the source of truth for approvals, tools, or session history
- treating ChatGPT/Codex subscription auth as generic OpenAI API access

## 6. Immediate Implications For The Belltower Backlog

The most valuable next implementation slices suggested by this review are:

1. message queuing and interrupt semantics
   - strongest direct gap relative to `pi` and Hermes

2. richer slash-command system
   - `/resume`, `/tools`, `/usage`, `/export`, `/mcp`, `/settings`
   - central registry and argument completion

3. branch/tree operator flows
   - closer to `pi`'s `/tree` and `/fork` semantics

4. auth-store evolution
   - add storage mode support and later migration/import helpers

5. protocol and app-server hardening
   - clearer lifecycle docs, handshake expectations, and event surfaces

6. sandbox and approval policy UX
   - make policy visible and selectable, not hidden in implementation

## 7. Human Validation Checklist From These Case Studies

Use these checks as we continue implementing:

### Operator UX

- `belltower` starts a new session cleanly
- `belltower resume` shows a useful picker
- `/` opens a stable, non-jumping command menu
- the TUI remains usable while the model is working

### Agent Loop

- a tool approval pauses one turn
- approving once resumes and completes the same turn
- tool results are fed back into the agent before the final assistant answer
- pending approval state is event-driven and clears correctly

### Queueing

- sending input while the model is working queues it predictably
- queued messages can be inspected and cancelled
- interrupt behavior is clear and recoverable

### Auth

- `login`, `doctor`, `status`, and `model` agree on what is actually usable
- credential source is visible
- local server auth remains enforced

### Telemetry

- every streamed response is durable as both raw chunks and structured events
- OTLP/OpenInference export reflects the stored session truth

## 8. Decision Rule Going Forward

When a proposed feature, refactor, or UX change comes up, ask:

1. did `pi`, Hermes, or Codex already solve this well?
2. if yes, are we copying:
   - behavior
   - lifecycle invariant
   - operator UX
   - subsystem boundary
3. does the adaptation preserve Belltower's core architecture:
   - canonical session/event store
   - protocol-first client/server split
   - OpenInference-first event model
   - telemetry-derived export model

If the answer to the last question is no, we should not copy it directly.
