# Belltower Architecture Overview

## Design Document — v0.3

### Summary

Belltower is a standalone Rust agent harness with a canonical HTTP/SSE boundary. It is built server-first: the server defines the control-plane contract, `bt-client` is the only supported Rust consumer surface, and the TUI uses the same protocol path as any future GUI.

The first implementation pass establishes the foundations that every later feature depends on:

- `bt-core` for shared types, traits, and Belltower-owned event envelopes with OI-compatible projection
- `bt-protocol` for versioned DTOs, typed errors, and SSE envelopes
- `bt-client` for authenticated local/server transport
- `bt-session` for SQLite persistence, replay, and projection-backed queries
- `bt-runtime` for composition of sessions, approvals, control queues, connections, and instructions

Everything else builds on top of that contract.

## Core Principles

### 1. Session-As-Telemetry

A session is its telemetry log. The canonical record is the Belltower-owned SQLite store, not exported OTLP spans, not UI state, and not a derived message list.

Each event carries the fields needed to align with OpenInference semantics from day one:

- trace and span identifiers
- optional turn identifiers for grouping runtime work
- span kind (`AGENT`, `LLM`, `TOOL`, `CHAIN`, `SESSION`)
- provider and model identifiers
- token and cost fields
- tool metadata
- structured input/output attributes

OpenTelemetry export is additive. `bt-otel` reads stored session data and transforms it into OTLP/OpenInference spans; it does not replace the session store.

The internal rule is:

- typed `EventEnvelope`s are the canonical domain-event bus
- SQLite is the canonical durable store
- SSE is a projection of committed events
- `tracing` and OTel are mirrors or export layers over committed events, not the primary source of truth
- the runtime tracing bridge mirrors committed events after `seq_id` assignment and truncates large payload fields instead of replacing the canonical store
- the live execution and export views are derived from the canonical event log: Belltower sessions contain turns, and OTLP/OpenInference export emits one trace per turn with `llm_call` and `tool_execution` spans underneath it

The write path is sacred:

1. construct a typed `EventEnvelope`
2. commit it to SQLite and assign `seq_id`
3. fan out the committed envelope to post-commit projections such as SSE and the tracing mirror

Neither SSE nor `tracing` is allowed to invent canonical events.
The SSE/UI path must be driven by committed envelopes, not by the tracing mirror.

This keeps replay, branching, and high-fidelity content durability under Belltower's control while still allowing first-class observability interop.

This also needs to scale beyond a single interactive thread.

Belltower should be able to understand work performed by related sessions, not only the currently active one.
That means the canonical model must be able to represent and inspect:

- parent and child session relationships
- delegation or spawned-agent lineage
- cross-session provenance such as "this branch/session was created from that session/turn"
- the operator-visible history of what each related session did

Even before subagent execution is implemented, the data model and inspection surfaces should be prepared for session graphs rather than isolated session logs.

The intended execution model for future delegated work is documented in:

- [`./subagents-and-session-graphs.md`](./subagents-and-session-graphs.md)
- [`./participants-and-collaboration.md`](./participants-and-collaboration.md)
- [`./memory.md`](./memory.md)
- [`./reactions-and-playbooks.md`](./reactions-and-playbooks.md)
- [`./provider-output-and-transcript-model.md`](./provider-output-and-transcript-model.md)
- [`./tool-system-and-normalization.md`](./tool-system-and-normalization.md)
- [`./hooks-and-lifecycle.md`](./hooks-and-lifecycle.md)
- [`./signals-and-ingress.md`](./signals-and-ingress.md)
- [`./claude-code-lessons-and-adoption.md`](./claude-code-lessons-and-adoption.md)
- [`./session-bundles-and-trace-sync.md`](./session-bundles-and-trace-sync.md)
- [`./datasets-and-training-artifacts.md`](./datasets-and-training-artifacts.md)
- [`./event-taxonomy-and-write-path.md`](./event-taxonomy-and-write-path.md)

That document fixes one important boundary:

- branch lineage is for speculative continuation inside one session
- child-session lineage is for delegated or spawned work across sessions

Turn lifecycle is explicit in the canonical event taxonomy:

- `turn.started`
- `completion.requested`
- `completion.chunk`
- `tool.call.requested`
- `tool.approval.*`
- `tool.execution.finished`
- `completion.finished`
- `turn.finished`

That taxonomy is the real cross-crate contract.
Changes to canonical event kinds should be treated like protocol changes because they affect storage, SSE, tracing, export, and TUI rendering together.

### 2. Server-First Transport

The canonical interface is `bt-protocol` over HTTP/SSE.

- `bt-server` owns the public API.
- `bt-client` is the only Rust client surface.
- `bt-tui` always talks through `bt-client`.
- interactive TUI mode may auto-start a colocated `bt-server`, but never bypasses the protocol.

Embedded runtime mode is intentionally excluded from v1 to avoid split execution paths.

### 3. Raw Stream Durability

Provider responses are persisted twice:

- canonical structured events in the event log
- raw provider stream chunks in a dedicated durability table

This preserves replay/debugging fidelity and allows reparsing when providers change stream behavior or the harness improves its parser.

### 4. Projection-Backed Reads

The event log is append-only, but read paths do not scan serialized enums. Belltower maintains projection tables for:

- messages
- approvals
- tool runs
- branch heads
- cost summaries

Replay and export can still reconstruct from the canonical stream, but operational reads are projection-backed.

## Workspace Structure

### Core Stable

- `bt-core`
- `bt-protocol`
- `bt-client`
- `bt-session`
- `bt-runtime`
- `bt-agent`
- `bt-providers`
- `bt-tools`
- `bt-context`
- `bt-auth`
- `bt-mcp`
- `bt-server`
- `bt-tui`

### Optional Standalone Expansion

- `bt-models`
- `bt-otel`

### Experimental Later

- `bt-improve`

`bt-improve` remains feature-gated and outside the first compatibility promise.

## Crate Responsibilities

### `bt-core`

Defines:

- IDs, messages, branches, sessions
- Belltower-owned event envelopes and payloads with OI-compatible attribute coverage
- config types
- provider/tool/auth/model traits
- approval evaluator abstractions
- shared errors and result types

`bt-core` must stay independent of app-specific assumptions and cannot depend on Belltower consumers.

### `bt-protocol`

Defines the public wire contract:

- versioned DTOs
- typed error envelopes
- monotonic SSE envelopes
- protocol version negotiation
- `Last-Event-ID` replay semantics

Generated TypeScript SDK work is deferred until the protocol surface stabilizes through dogfooding.

### `bt-client`

Owns:

- token discovery
- authorization header construction
- typed API helpers
- reconnect headers including `Last-Event-ID`

### `bt-session`

Owns the canonical SQLite store:

- WAL mode
- append-only events
- raw chunk durability
- branch tracking
- projection refresh
- replay helpers
- export helpers

The storage layer must support concurrent sessions while ensuring writes stay serialized.
That means one canonical writer path per database, with global `seq_id` ordering across sessions even when multiple sessions are active concurrently.

### `bt-runtime`

Owns composition:

- session services
- branch services
- approval state
- cancel and steer queues
- persisted session budget state and restart-safe budget accounting
- connection registry
- instruction resolution
- session store integration
- context preparation orchestration
- canonical compaction persistence

### `bt-agent`

Remains pure:

- turn loop
- tool-result re-entry
- retries
- per-turn budget and completion semantics inside the typed turn loop
- steer consumption

It must not absorb server, auth, or storage lifecycle logic.
It should not depend on `bt-session`, `bt-runtime`, `bt-server`, or `bt-auth`, and that boundary is now enforced mechanically with a dependency-closure guard that keeps `bt-agent` limited to `bt-core` and `bt-tools` among workspace crates.

The runtime-facing contract from `bt-agent` should stay typed and storage-free.
The agent emits turn outcomes and messages/tool results/chunks for the runtime to persist; it does not write directly to the canonical store.

### `bt-context`

Owns request shaping:

- managed harness prompt assembly
- runtime-context prompt sections
- tool-guidance prompt sections
- provider prompt overlays
- token-budgeting heuristics
- compaction algorithms and summaries

v1 prompt assembly is intentionally small and explicit:

- managed markdown prompt assets from `data/prompts/`
- generated runtime context including the session working directory (`cwd`)
- generated tool guidance from the actual visible tool specs
- global and project markdown instruction overlays
- active plan injection

At the foundation layer, instruction provenance and reconstructability
touch both `bt-context` and `bt-runtime`:

- `bt-context` owns the effective prompt/input assembly pieces
- `bt-runtime` owns the orchestration path that requests and records them
- `bt-session` owns the durable record needed for historical reconstruction

### Runtime Boundary Warnings

`bt-runtime` is a composition crate, not a catch-all.

These concerns should not grow inside it:

- subagent spawn or workflow orchestration logic
- worktree lifecycle management
- export formatting
- provider-specific protocol behavior

If `bt-runtime` starts accumulating provider-specific branches beyond trait dispatch, the abstraction is leaking upward.

## Auth And Local Security

Local API access is not anonymous.

- `bt-server` generates a random startup token.
- the token is written to an endpoint-scoped record under `~/.config/belltower/server-auth/` with owner-only permissions
- every HTTP and SSE request except `GET /health` requires `Authorization: Bearer <token>`
- `bt-client` owns token discovery and request authentication

This keeps the zero-config local UX while avoiding an unauthenticated canonical API.

The auth material is scoped to the server instance the client is talking to:

- the endpoint URL determines the preferred local auth record
- `bt-client` and the launcher both resolve that endpoint-scoped record before falling back to the legacy shared token path for compatibility
- long-lived clients cache the resolved token so another server startup cannot silently rotate auth out from under an already-running client

## Catalog And Config

Belltower ships embedded data files for:

- provider defaults
- pricing
- context windows
- model recommendations

These ship with the binary and are overlaid by:

1. built-in defaults
2. global config at `~/.config/belltower/config.toml`
3. project config at `.belltower/config.toml`

This keeps the default provider catalog data-driven instead of hardcoding it in Rust logic.
Override files are strict: unknown keys fail with their full configuration path rather than silently falling back to embedded defaults.

## Instructions, Prompt Assets, And Skills

v1 includes a minimal markdown-based instruction system:

- managed built-in harness prompt assets
- global skill directory
- project skill directory
- no hook system yet
- deterministic composition order
- on-demand loading to preserve prompt/cache efficiency

The built-in harness layer is separate from user/project instruction overlays:

- `data/prompts/00-core.md` is the required core harness prompt
- `data/prompts/provider/*.md` provides optional provider-family overlays
- global and project markdown instructions remain composable overlays loaded at runtime

Hooks are a separate later concept.

They should eventually exist as runtime or plugin lifecycle callbacks around session, LLM, tool, and error boundaries.
They are not the same thing as tools or skills, and they should not be implemented by pushing callback logic into `bt-agent`.

See:

- [`./hooks-and-lifecycle.md`](./hooks-and-lifecycle.md)

Cross-session semantic memory is also a separate later subsystem.

It should integrate through provider bindings, tools, signals, and reactions rather than by mutating the session model itself.

See:

- [`./memory.md`](./memory.md)

This v1 prompt and skill stack is enough to support structured prompt loading without introducing a larger plugin or hook system yet.

The next architectural layer on top of this is a real skill system and a reviewable self-improvement workflow:

- [`./skills-and-self-improvement.md`](./skills-and-self-improvement.md)

## Storage Model

The canonical store uses:

- `sessions`
- `branches`
- `events`
- `raw_chunks`
- message, approval, tool, branch-head, and cost projections

Each event receives a monotonic `seq_id`. SSE reconnection uses `Last-Event-ID` mapped to that sequence number. Branching is explicit and replayable; branch heads are tracked separately from the append-only stream. Rewind is represented as an append-only child branch from a selected parent event boundary, not as mutation of prior events.

Raw chunk durability needs first-class query ergonomics, not just bytes on disk.
The schema and indexes should support:

- all raw chunks for a given turn on a given branch
- all raw chunks for a particular LLM call within a session
- comparison between the finalized structured projection and raw provider stream data for the same turn

Session lineage should also be explicit and replayable.
The store should be able to answer questions such as:

- which session spawned or resumed which other session
- which turn or branch a child session came from
- what happened in descendant sessions after delegation
- how a higher-level autonomous workflow relates to the underlying session graph

If SQLite is unavailable, canonical turn progression should stop instead of silently degrading into a non-durable mode.
Belltower's main value disappears if the canonical store becomes optional.

## OpenInference And OpenTelemetry

OpenInference alignment is part of the core design, not a later addon.

- event payloads and attributes carry the information needed for OI-compatible LLM, tool, chain, and agent spans
- the runtime/store remain the source of truth
- `bt-otel` is responsible for mapping stored Belltower-owned fields into OTLP/OpenInference backends

The alignment is structural, not nominal.
Internal payload fields should stay Belltower-owned and stable even if OI semantic-convention names evolve.
Mapping to specific OI attribute names belongs in `bt-otel`.

This means Belltower stays useful even with no telemetry collector configured, while still supporting export to Phoenix or any OTLP consumer later.
It also means later dataset, replay, and training-artifact workflows can be built from the same canonical store instead of inventing a separate trajectory runtime.

## Scope Locks

In v1:

- steer is first-class
- branching is first-class
- raw chunk persistence is first-class
- OpenInference alignment is first-class
- standalone provider catalog/config layering is first-class

Deferred to v2:

- session interop import/export with other runtimes
- native inference backends
- subagent execution
- plugin hosting beyond MCP/custom tools

Even though subagent execution is deferred, lineage and observability requirements for future multi-session workflows are not deferred.
The stable architecture should already preserve enough structure to inspect related sessions coherently once they exist.

Experimental later:

- self-improvement via `bt-improve`

## Initial Delivery Bar

The first coding pass implements:

- docs rewrite
- workspace/bootstrap
- `bt-core`
- `bt-protocol`
- `bt-client`
- `bt-session`
- `bt-runtime`

Later slices build providers, tools, context management, MCP, server, TUI, local model workflows, OTLP export, and isolated self-improvement on top of that base.
