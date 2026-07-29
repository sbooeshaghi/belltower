# Core, Protocol, and Server

This document covers the control-plane foundation of Belltower:

- `bt-core`
- `bt-protocol`
- `bt-client`
- `bt-server`

The closest case-study analogues are:

- `@mariozechner/pi-ai` for the external model/tool-facing contract shape
- `@mariozechner/pi-agent-core` for event-oriented execution semantics
- Hermes's API server and architecture docs for operator-facing protocol expectations

## Purpose

These crates define the public and internal contract that every other subsystem depends on.

If this layer is wrong, later work on the TUI, providers, tools, MCP, and telemetry becomes expensive to unwind.

## Crate Responsibilities

### `bt-core`

Owns:

- shared IDs
- event and message types
- OpenInference-aligned event envelopes
- provider/tool/auth/model traits
- shared config and error types

Design rule:

- `bt-core` must stay independent of app-specific behavior and UI concerns

### `bt-protocol`

Owns the canonical wire contract:

- request/response DTOs
- typed error envelopes
- SSE envelope shape
- protocol version negotiation
- replay semantics via `Last-Event-ID`

Design rule:

- if a Rust client or future GUI needs a surface, it should appear here first

### `bt-client`

Owns:

- authenticated request construction
- typed Rust access to the server
- SSE reconnect behavior
- explicit auth-mode constructors for:
  - remote clients with a bearer token
  - launcher-owned local flows that discover endpoint-scoped auth by base URL

Design rule:

- Rust consumers should not manually reconstruct protocol requests when
  `bt-client` already owns that behavior
- the discovered-auth path is a launcher/local-host convenience, not the
  baseline remote-client contract

### `bt-server`

Owns:

- the canonical HTTP/SSE API
- route-level auth enforcement
- health and bootstrap behavior
- server metadata and capability reporting
- session/event/control-plane endpoints

Design rule:

- the TUI and launcher may start the server, but must not bypass it

## Core Concepts

## 1. Server-First Transport

Belltower is not a local-only embedded runtime with a server bolted on later.

The control plane is:

1. `bt-server` publishes the API
2. `bt-client` consumes the API
3. `bt-tui` uses `bt-client`
4. future GUIs or apps consume the same protocol

This is the main place where Belltower intentionally diverges from the "CLI first, server later" pattern common in agent tools.

## 2. Typed Protocol Contract

The protocol must make the following things explicit:

- session creation and lookup
- canonical session budget DTO shapes for creation and later control updates
- branch-addressed message submission and operator/control mutations
- approval actions, including durable request snapshots and separate
  resolution evidence in inspection/event DTOs
- branch creation and switching, including optional event-boundary forks via
  `CreateBranchRequest.from_event_id`
- event replay
- model backends, recommendations, and connection-scoped model inventories
- provider and readiness status
- MCP status
- MCP reload control
- export and inspection surfaces
- workflow and lineage inspection

This should be versioned and golden-testable.

The shape rule matters here: session autonomy limits should appear as one
canonical `BudgetConfig`-backed DTO surface in `bt-protocol`, not as multiple
ad hoc scalar groups repeated across clients. Runtime enforcement can evolve on
top of that shared shape without changing the consumer contract.

The current server/runtime contract now uses that shape directly on session
creation: `POST /sessions` may persist an initial `BudgetConfig`, and later
turn completion atomically records runtime-owned `budget.checkpoint`, optional
`session.cancelled { reason: "budget_exhausted" }`, terminal evidence, and
`turn.finished` events without introducing a server-local budget model. The
server transports the resulting state and never owns a second budget predicate.

## 3. Local API Security

The canonical local API is not anonymous.

The current design requires:

- a generated bearer token
- storage in the Belltower config directory
- protection of all non-health routes
- automatic use by `bt-client`

This keeps the local workflow simple without normalizing an unauthenticated control plane.

The local auth contract is now server-instance-scoped:

- `bt-server` writes a token record for its own endpoint under `~/.config/belltower/server-auth/`
- the record name is derived from the server URL so multiple local servers do not overwrite each other
- `bt-client` discovers auth by server URL first and only falls back to the legacy shared token path for compatibility
- long-lived local clients reread endpoint token files so server restarts and token rotation do not strand them with stale bearer tokens
- the launcher uses the same endpoint-first lookup order when probing an already-running server

This removes the main `401 Unauthorized` failure mode that came from rotating one globally shared token file while another local server was already in use.

## Key Flows

## Health and Startup

1. `belltower` loads config and determines whether to auto-start `bt-server`
2. `bt-server` exposes `GET /health` without auth
3. `bt-server` exposes `GET /server/info` with bearer auth for richer metadata
   and capability reporting
4. before launching the TUI, the launcher waits for health plus a successful
   endpoint-scoped bearer-token probe against an authenticated protocol route
5. all other routes require bearer auth
6. `bt-client` loads the token and talks to the server

## Session Control

1. create or resume session through `bt-client`
2. open SSE stream
3. send a message or control action; message ingress returns a typed
   `dispatched` or `queued { position }` outcome from the runtime-owned atomic
   admission transition rather than a client-side inference
4. receive ordered events with monotonic IDs
5. reconnect using `Last-Event-ID` if interrupted

Dispatch is asynchronous: send/approve/answer acknowledge durable admission
(202) and the turn runs on a detached server task, so a client disconnect can
never abort an admitted turn. Turn outcomes are observed through the event
stream or the turns/queue inspection routes, not the dispatching POST's
status; a watchdog force-fails a turn whose task panics, and startup recovery
terminalizes turns interrupted by a crash. Clients that need the outcome
synchronously (headless runs, tests) use
`BelltowerClient::wait_for_session_settle`.

Branch identity is part of the write contract, not server-side ambient state.
`UpdateSessionRequest`, `UpdateSessionBudgetRequest`,
`CancelSessionRequest`, `SteerSessionRequest`,
`RecordOperatorCommandRequest`, and `RunShellCommandRequest` require a
`branch_id`, just as message and compaction requests do. Handlers validate that
the branch belongs to the path session and pass the same identity into runtime
events. Settings and budget remain session-scoped projections; their branch is
canonical provenance for the operator invocation. While a turn is active,
cancel and steer additionally require the requested branch to own that turn; a
mismatch returns a protocol error before any event is appended. This keeps the
canonical event branch equal to the work that was actually controlled.

Extended-tool sessions also register four server-hosted model tools over the
same runtime/store boundary:

- `spawn_agent` creates and dispatches one bounded child session, optionally on
  a different configured connection and model
- the operator route `POST /sessions/{id}/spawn` has the same dispatch
  contract: the objective is delivered as a wake instruction and the child
  starts immediately; `dispatch: false` opts into a prepared-but-idle child
- `send_agent_message` records a typed message to any session in the same
  lineage tree (parent, child, sibling, or deeper relative) and may wake an
  idle destination; `target_branch_id` is required, and replies must name the
  exact branch from the immutable reverse message edge; parents are not copied
  on peer traffic
- `list_agents` returns canonical workflow lineage plus recent mailbox
  records, optionally for another same-tree session via `target_session_id`
- `wait_agent` performs a bounded mailbox wait without joining or cancelling
  another session

These are tool adapters, not an alternate control plane. The server does not
own lineage, mailbox status, message claim, settings selection, or turn
continuation semantics. `spawn_agent` is always approval-gated and requires
explicit shared-workspace consent until isolated-worktree lifecycle exists.
Mailbox events are ordinary committed events, so SSE replay and portable
session bundles carry them without a tool-specific transport format.

Server handlers stay thin in this flow. They may provide runtime adapters for
tool-registry construction and provider construction, but turn semantics remain
runtime-owned. Budget eligibility is part of the runtime/store admission or
continuation claim, not a separate HTTP precheck, and successful admission
returns an opaque runtime-issued turn capability bound to the admitted session
and branch. Approval and pending-input endpoints also delegate one exact
request-sequence claim to runtime/store; the server does not resolve the
decision and then race a separate turn start. Budget configuration is also
runtime/store-owned and is accepted only while the session is idle. The runtime validates the executable
provider/auth/model path before recording model-visible context mutations for
that turn. Failed provider construction or credential preflight is surfaced as
durable runtime failure state, not as a server-local partial continuation.

## Design Commitments

- `bt-protocol` is the only supported Rust consumer boundary
- typed errors are part of the public contract
- missing sessions, branches, and tool calls should surface as `404 not_found`
  envelopes rather than leaking through `runtime/invalid_state`
- SSE replay behavior is not best-effort; it is a required feature
- protocol compatibility needs a real promise once v1 stabilizes
- richer server metadata belongs in a typed protocol route rather than ad hoc
  launcher probing

## Stabilization Discipline

For the current release push, this layer is under protocol stabilization.

That means:

- route names, DTO shape, typed errors, and SSE envelopes are now guarded intentionally
- pre-release cleanup is still allowed, but only intentionally and together with fixture/test updates
- the point is not to stop development
- the point is to let TUI, auth/models, MCP, and export work continue without accidental wire drift

The operational rule lives in [`../development/protocol-stabilization.md`](../development/protocol-stabilization.md).

## Human Checkpoints

Use these tests while developing this layer:

1. `GET /health` succeeds without auth
2. protected routes reject missing or bad bearer tokens
3. a session stream can reconnect with `Last-Event-ID`
4. `bt-client` can consume the same session/control surfaces the TUI uses
5. protocol changes can be guarded with golden tests

## Current Priorities

The next improvements for this layer are:

- maintain the new protocol fixture and compatibility harness as the route/DTO surface evolves
- sharper bootstrap and session-list surfaces for the TUI
- continued clarification of remote-consumer versus launcher/local-bootstrap
  seams, following [`../development/consumer-modes.md`](../development/consumer-modes.md)
- clearer documentation coverage for the current route surface, especially readiness, workflow, and MCP
- live-client validation against real export/collector consumers
