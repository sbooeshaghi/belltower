# Memory Subsystem Spec

This document defines the implementation-facing specification for Belltower's memory subsystem.

Read it with:

- [`./memory.md`](./memory.md)
- [`./signals-and-ingress.md`](./signals-and-ingress.md)
- [`./reactions-and-playbooks.md`](./reactions-and-playbooks.md)
- [`./tool-policy.md`](./tool-policy.md)
- [`./hooks-and-lifecycle.md`](./hooks-and-lifecycle.md)
- [`./event-taxonomy-and-write-path.md`](./event-taxonomy-and-write-path.md)
- [`../subsystems/session-runtime-and-agent.md`](../subsystems/session-runtime-and-agent.md)
- [`../subsystems/tools-context-and-approvals.md`](../subsystems/tools-context-and-approvals.md)

This document is more concrete than [`./memory.md`](./memory.md).
That document explains the architectural commitments and trust boundaries.
This document fixes the intended subsystem shape, crate boundaries, provider contract, tool exposure model, and first implementation slice.

Tool execution policy for memory tools is defined more generally in:

- [`./tool-policy.md`](./tool-policy.md)

## Status

This is a design and implementation target, not a statement that the subsystem already exists in code.

The intent is:

- provider-neutral memory first
- built-in markdown wiki provider second
- Semaphora integration after the core contract is real

The markdown provider is the first manifestation of memory.
It is not the definition of the subsystem.

## Summary

Belltower memory is a first-class, provider-neutral, cross-session harness subsystem.

It exists to support:

- semantic memory that spans sessions and workflows
- explicit agent and operator access to memory through tools
- explicit attachment of bounded memory context bundles to turns
- canonical inspection of when memory influenced work
- future normalization of provider-originated events into Signals

The critical architectural split is:

- `bt-session` owns the operational record of how memory was used in a session
- `bt-memory` owns bindings, provider access, and cross-session memory integration
- the provider owns the semantic store itself

## Design Goals

The subsystem should:

- remain provider-neutral at the API and type level
- support cross-session and cross-workflow memory bindings
- keep memory influence explicit and inspectable
- make memory available to the agent through ordinary tool use
- allow bounded pre-turn context attachment under runtime policy
- preserve a clean path for future Signals/Reactions integration

The subsystem should not:

- treat markdown as the universal memory model
- copy provider semantic graphs into Belltower-owned storage
- collapse memory into the session event log
- silently inject provider state into prompts
- push provider semantics into `bt-server` or `bt-agent`

## Ownership and Crate Boundaries

### `bt-memory`

`bt-memory` should be added as a new first-class crate.

It owns:

- provider-neutral memory types
- provider traits
- binding registry and scope resolution support
- Belltower-owned memory registry state
- provider loading and inspection surfaces
- built-in providers
- later provider-to-signal adapter seams

It does not own:

- canonical session telemetry
- public HTTP/SSE transport
- agent loop semantics
- provider-native semantic truth

### `bt-runtime`

`bt-runtime` owns runtime orchestration around memory.

It should own:

- `MemoryService` composition
- binding visibility resolution for the active runtime context
- deterministic pre-turn context fetch and attachment
- reaction-owned memory pre-actions

It should not own:

- provider-specific memory semantics
- provider-native indexing
- ad hoc memory registries outside `bt-memory`

### `bt-session`

`bt-session` owns the operational trail of memory usage inside a session.

It should store:

- canonical events describing Belltower-owned memory actions
- turn-linked references to memory objects that influenced work
- bounded attached context bundle snapshots when memory affected a turn

It should not store:

- the provider's semantic graph
- cross-session binding registry state

### `bt-server`

`bt-server` exposes memory through protocol routes and tool-backed surfaces only.

It should not become the place where memory semantics live.

### `bt-tools`

`bt-tools` should expose the canonical memory tool family.

### `bt-agent`

`bt-agent` remains provider-agnostic.

It should only see:

- memory tools
- explicit attached memory context bundles

It must not know how any provider stores or indexes memory.

## Scope and Binding Model

Memory bindings are cross-session by default.

A session may read or write a memory binding without owning it.

### Binding scopes

Bindings should support at least:

- `global`
- `project`
- `workflow`
- `session_visible`

Interpretation:

- `global`
  - available across projects for the local runtime or user
- `project`
  - shared by sessions in one project
- `workflow`
  - shared by sessions or reactions participating in one named workflow
- `session_visible`
  - visible to one session under runtime policy, but not session-owned

### Binding selection rules

- explicit `binding_id` always wins
- otherwise the runtime resolves visible bindings by scope and enabled state
- one default binding may exist per effective scope class
- sessions inherit visibility, not ownership

## Configuration and Belltower-Owned State

### Project configuration

Project bindings should live in:

- `.belltower/memory.toml`

Example:

```toml
[[bindings]]
id = "project_memory"
provider = "wiki_markdown"
scope = "project"
root = ".belltower/memory/project_memory"
default = true
enabled = true
```

### Belltower-owned registry state

Belltower-owned cross-session memory state should live in a dedicated memory registry store such as:

- `.belltower/memory-state.sqlite`

This store should own:

- binding registry state
- binding health/status cache
- resource registration mappings
- provider connection metadata references
- later provider cursors or checkpoint state

It should not own:

- provider-native semantic objects
- provider-native indexes
- provider-native review queues except as references

### Provider-local state

Provider-local state should live under provider-controlled paths such as:

- `.belltower/memory/<binding_id>/...`

The exact layout is provider-specific.

## Provider-Neutral Type Model

The core contract should not be built around markdown pages.

The provider-neutral vocabulary should center on:

- bindings
- resources
- memory object references
- memory object views
- relations
- citations
- context bundles

Suggested core types:

- `MemoryBinding`
- `MemoryObjectRef`
- `MemoryResourceRef`
- `RegisteredResource`
- `MemorySearchResult`
- `MemoryObjectView`
- `MemoryRelation`
- `MemoryCitation`
- `MemoryContextBundle`

The key abstraction is:

- provider-neutral `view`

Not:

- provider-specific `page`

A wiki provider may expose pages and backlinks inside `MemoryObjectView`.
A graph provider may expose nodes and neighborhoods.
A review provider may expose tasks, evidence, and status.

## Provider Contract

The initial provider trait should look like:

```rust
pub trait MemoryProvider: Send + Sync {
    fn kind(&self) -> &'static str;

    async fn status(&self, req: ProviderStatusRequest) -> Result<ProviderStatus>;
    async fn register_resource(&self, req: RegisterResourceRequest) -> Result<RegisteredResource>;
    async fn write(&self, req: MemoryWriteRequest) -> Result<MemoryWriteResult>;
    async fn search(&self, req: MemorySearchRequest) -> Result<MemorySearchResponse>;
    async fn view(&self, req: MemoryViewRequest) -> Result<MemoryObjectView>;
    async fn fetch_context(&self, req: MemoryContextRequest) -> Result<MemoryContextBundle>;
}
```

The provider contract should support:

- ordinary reads
- ordinary writes
- bounded context assembly
- resource registration
- lightweight health inspection

It should not assume:

- one graph shape
- one backing database
- one context-assembly algorithm
- one watch or signal model

## Agent and Operator Tool Exposure

Memory should reach the agent as ordinary harness tools.

The canonical tool family should begin with:

- `memory.list_bindings`
- `memory.provider_status`
- `memory.search`
- `memory.view`
- `memory.register_resource`
- `memory.write`
- `memory.fetch_context`

This spec does not require a separate canonical `recall` built-in.

If Belltower later introduces `recall`, it should be treated as a convenience
wrapper over the `memory.*` family rather than the foundational contract.

### Agent visibility rules

The runtime should expose memory tools when the session can see at least one enabled binding.

The agent should not receive hidden memory state by default.

The allowed ways memory becomes model-visible are:

- explicit in-turn tool use
- explicit bounded context attachment by the runtime

### Operator surfaces

Operators should be able to reach the same capabilities through:

- slash commands
- API surfaces
- future UI inspection surfaces

Those should reuse the same `MemoryService` path rather than inventing a second control plane.

### Default tool-policy expectations

Every memory tool should declare the same core policy dimensions as other
canonical tools:

- risk classification
- mutability class
- approval behavior
- concurrency behavior
- interrupt behavior
- any boundedness or provenance requirements

The exact enum names may evolve, but the v1 intent should be:

- `memory.list_bindings`
  - read-only
  - low risk
  - auto-approved
  - concurrency-safe
- `memory.provider_status`
  - read-only
  - low risk
  - auto-approved
  - concurrency-safe
- `memory.search`
  - read-only
  - low risk
  - auto-approved
  - concurrency-safe
- `memory.view`
  - read-only
  - low risk
  - auto-approved
  - concurrency-safe
- `memory.fetch_context`
  - read-only but policy-bounded
  - auto-approved only within runtime-enforced bundle limits
  - must preserve citations or explicit references when available
  - must not silently attach unbounded provider state
- `memory.register_resource`
  - mutates Belltower-owned registry state
  - medium risk by default
  - may be auto-approved for trusted local project outputs and `bt://...` artifacts
  - may require approval for broader or externally scoped registrations
- `memory.write`
  - durable semantic mutation
  - approval-sensitive by default
  - serialized per binding
  - should remain more tightly governed than ordinary read tools until stronger trust policy exists

These defaults should be implemented through the general tool-policy layer,
not as memory-specific runtime special cases.

## Runtime Execution Modes

Memory should run in three legitimate modes.

### 1. Explicit in-turn tool use

The model chooses to call:

- `memory.search`
- `memory.view`
- `memory.write`
- `memory.register_resource`
- `memory.fetch_context`

This is the default mode and should cover most reading and writing.

### 2. Deterministic pre-turn setup

The runtime may fetch a bounded context bundle before model execution begins.

Examples:

- a reaction fetches related memory after a signal matches
- a future lifecycle boundary loads a small working-memory snapshot

This setup is runtime-owned and must be explicit and inspectable.

### 3. Operator-initiated memory actions

The operator may inspect or modify memory directly through UI or API surfaces.

These actions should still pass through `bt-memory` and the normal runtime/service path.

## Canonical Session Logging

Memory use inside a session must appear in the operational record.

### Explicit tool use

If the agent uses memory through a tool, the operational trail should be captured through the normal canonical tool event path:

- `tool.call.requested`
- `tool.execution.finished`

That already records:

- which tool was called
- which arguments were used
- which result came back
- when the call happened in the turn

This is the correct logging path for ordinary agent reads and writes.

### Memory-specific canonical events

Add memory-specific events only where Belltower itself owns the behavior and generic tool events are not enough.

Likely families:

- `memory.binding.registered`
- `memory.binding.updated`
- `memory.binding.disabled`
- `memory.resource.registered`
- `memory.context.requested`
- `memory.context.attached`

Rules:

- do not introduce a redundant `memory.write.called` event for ordinary explicit tool use
- do emit `memory.resource.registered` when Belltower records a durable resource-to-provider mapping
- do emit `memory.context.requested` and `memory.context.attached` for runtime-owned pre-turn attachment

### Attached context durability

If a memory bundle influenced a turn, Belltower should durably preserve:

- the binding that supplied it
- a compact retrieval request summary
- the trigger source
  - explicit tool call
  - reaction id
  - future hook id
- the bundle budget or boundedness settings used
- the provider object references involved
- the citations or evidence handles when available
- a bounded snapshot of the attached bundle

When the provider can supply it cleanly, Belltower should also preserve:

- provider revision, cursor, or equivalent assembly version info

The system should not block v1 on universal provider revision support.

This is required for replay, inspection, and trust.

## Storage Model Split

The ownership split should remain explicit.

### Provider-owned

The provider owns:

- semantic objects
- native indexes
- native graph shape
- native page or object assembly behavior
- native review or change semantics

### Belltower-owned

Belltower owns:

- bindings
- binding scope and visibility
- provider connection metadata references
- resource registration mappings
- explicit memory references used in work
- attached context snapshots
- later normalized memory-originated signals

Belltower should store references and bounded context, not a duplicate semantic graph.

## Built-In Provider

The first built-in provider should be:

- `wiki_markdown`

It should be implemented as one provider inside `bt-memory`, not as the subsystem API.

### Built-in provider requirements

- markdown files are the provider's canonical authored store
- a derived sqlite index may support search, links, and backlinks
- writes are serialized per binding
- derived indexes and backlinks are updated before the write returns
- the provider maps provider-neutral operations onto markdown semantics

The built-in provider is a contract test for the subsystem:

- provider loading
- binding resolution
- canonical memory tools
- explicit session logging of memory use
- context bundle attachment

It should not become an excuse to make all memory types look like wiki pages.

## Relation to Signals, Reactions, and Hooks

### Signals

Memory-originated external events should enter Belltower as Signals.

Examples:

- change notifications
- review tasks
- action or watch events

These are not hooks and not synthetic user prompts.

### Reactions

Reactions should be able to use memory through deterministic pre-actions such as:

- fetch bounded context
- register a signal-related resource
- attach citation bundles

Reactions own policy.
Memory owns memory access.

### Hooks

Hooks are relevant later, but they are not a prerequisite.

Useful future hook seams include:

- `on_session_start`
- `pre_llm_call`
- `post_tool_call`
- `pre_compact`
- `on_session_end`

Hook-driven memory behavior must remain bounded and inspectable.

## Failure Semantics

Provider failure must not break ordinary Belltower execution.

Rules:

- provider failure may fail a memory tool call
- provider failure may cause a reaction-owned pre-action to skip, notify, or block according to policy
- provider failure must not break session writes
- provider failure must not break signal logging
- provider failure must not silently degrade session durability

The Belltower-owned registry state is still part of Belltower's durable system.
It should not fall back to hidden non-durable behavior.

## First Implementation Slice

The first implementation slice should be:

1. add `bt-memory`
2. add binding loading from `.belltower/memory.toml`
3. add Belltower-owned registry storage for binding and resource mapping state
4. implement the built-in `wiki_markdown` provider
5. expose:
   - `memory.list_bindings`
   - `memory.provider_status`
   - `memory.search`
   - `memory.view`
6. expose:
   - `memory.register_resource`
   - `memory.write`
7. record explicit memory tool use through ordinary tool events
8. later add pre-turn context attachment plus:
   - `memory.context.requested`
   - `memory.context.attached`

### V1 write contract

The first `memory.write` operation should be narrow.

Use a provider-neutral operation such as:

- `document_upsert`

The built-in wiki provider maps that to markdown-backed document updates.
Other providers may map it differently.

This keeps the core API generic while allowing a simple first implementation.

## Validation Strategy

Validation should include:

- provider contract tests shared across providers
- binding resolution tests
- registry-store tests
- built-in provider search/view/write tests
- canonical event ordering tests for context attachment
- unchanged `bt-agent` purity tests

Manual validation should confirm:

- the agent can use memory through normal tool calls
- the resulting tool calls appear in the session log
- attached context bundles are explicit and inspectable
- memory bindings survive across sessions
- sessions do not become the owner of the memory binding

## Open Decisions

These should remain explicit during implementation:

- whether `memory.fetch_context` is agent-visible in v1 or initially runtime/operator-only
- the exact `MemoryObjectView` field set for relations, excerpts, and citations
- how provider-specific escape hatches should be surfaced, if at all
- whether future provider-originated signal adapters live in `bt-memory` directly or a thin adjacent crate

## Implementation Rule

When code and this spec disagree, keep the higher-level architecture rule from [`./memory.md`](./memory.md):

- memory is provider-neutral and cross-session
- sessions record operational usage, not memory ownership
- the provider owns semantic truth
- Belltower owns bindings, runtime access, and inspectability
