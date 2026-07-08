# Memory

This document defines the intended shape of Belltower's memory subsystem.

Read it with:

- [`./overview.md`](./overview.md)
- [`./signals-and-ingress.md`](./signals-and-ingress.md)
- [`./reactions-and-playbooks.md`](./reactions-and-playbooks.md)
- [`./participants-and-collaboration.md`](./participants-and-collaboration.md)
- [`./tool-system-and-normalization.md`](./tool-system-and-normalization.md)
- [`./hooks-and-lifecycle.md`](./hooks-and-lifecycle.md)
- [`./event-taxonomy-and-write-path.md`](./event-taxonomy-and-write-path.md)
- [`./memory-subsystem-spec.md`](./memory-subsystem-spec.md)

For the implementation-facing subsystem contract, see:

- [`./memory-subsystem-spec.md`](./memory-subsystem-spec.md)

It exists because Belltower needs a first-class place for cross-session semantic memory without collapsing that concern into:

- the session event log
- ad hoc prompt injection
- a provider-specific sidecar hidden from the harness

The central architectural rule is:

- sessions are the operational record of what happened
- memory is the semantic record of what is known, believed, linked, reviewed, or worth revisiting across sessions

## Summary

Belltower should have a first-class Memory subsystem.

That subsystem is responsible for:

- binding one or more memory providers to Belltower scopes
- exposing a provider-neutral memory tool vocabulary to the agent and operator
- registering Belltower artifacts and resources into provider memory spaces
- retrieving bounded memory context bundles and citation bundles when policy requires them
- normalizing provider-originated change, review, and action events into Belltower signals
- preserving memory provenance, routing, and inspection surfaces inside Belltower

This layer should be generic enough to support:

- a tiny Belltower-built-in document-based working-memory provider
- Semaphora as the first real semantic memory provider
- future external memory systems

It should not require Belltower to absorb the provider's native semantic database.

## Why this needs to be first-class

Without a first-class memory subsystem, Belltower will tend toward one of three bad patterns:

- cross-session memory implemented as hidden prompt text
- each memory provider inventing its own ad hoc tools and runtime integration
- semantic memory being confused with the operational session log

All three make the harness harder to reason about.

Memory should therefore be treated as a harness-level concern in the same way that:

- tools
- approvals
- signals
- reactions
- sessions

are harness-level concerns.

The trust boundary should stay explicit:

- the memory provider owns semantic storage and provider-native semantics
- Belltower owns bindings, canonical signal ingress, routing, turn setup, and inspectability
- the agent sees memory through explicit tools or explicit context attachment, not hidden provider prompt injection

## Memory is not the session log

The distinction between session state and memory should stay strict.

Sessions are:

- turn-scoped
- operational
- append-only
- canonical for "what happened"

Memory is:

- cross-session and often cross-workflow
- semantic
- grounded in real objects and artifacts
- canonical for "what this work means" only inside the provider that owns that semantic store

Examples:

- a turn that generated a figure belongs in the session log
- the semantic claim that the figure supports a manuscript section belongs in memory
- a change event about the figure belongs in the memory provider first and then becomes a Belltower signal

## Core concepts

### Memory provider

A memory provider is a system that stores and serves memory on behalf of Belltower.

Examples:

- the built-in document-based working-memory provider
- a Semaphora server
- a future project-specific semantic store

### Memory binding

A memory binding is a configured instance of a provider attached to some Belltower scope.

Examples:

- one built-in working-memory provider for a project
- one Semaphora server bound to a manuscript project
- one Semaphora server shared by several workflows inside one project

Bindings are what users register.

### Registered resource

A registered resource is a real object that Belltower exposes to memory.

Examples:

- a draft section
- an emitted figure
- a local file
- a fetched page
- a `bt://session/...`, `bt://turn/...`, `bt://event/...`, or future `bt://artifact/...` object

### Memory reference

A memory reference is the Belltower-side handle for something the provider knows about.

Examples:

- a provider node id
- a page id
- a review task id
- a resource id or URI

Belltower should preserve these references, but not pretend they are the provider's canonical semantic store.

### Memory context bundle

A memory context bundle is the bounded payload Belltower may attach to agent work.

Examples:

- a page or neighborhood view
- a small set of grounded review items
- a citation bundle related to a changed figure
- a provider-generated summary with explicit references

The context bundle is the unit that becomes model-visible, not the entire memory graph.

### Citation bundle

A citation bundle is the grounded support Belltower can show when memory affects agent work.

At minimum it should preserve:

- provider identity
- provider object references
- grounded resource references
- excerpts or evidence handles when available

## Functional requirements

The memory subsystem should support at least the following.

### 1. Cross-session semantic memory

Memory must outlive any one session and remain useful across related sessions and workflows.

### 2. Grounding in real objects

Memory should be able to ground itself in:

- files
- figures
- fetched pages
- attachments
- `bt://...` session artifacts
- provider-native resources

### 3. Provider-neutral operator and agent tools

Belltower should expose a small canonical memory tool vocabulary from day one.

The exact names can evolve, but the capability family should look like:

- `memory.register_resource`
- `memory.write`
- `memory.search`
- `memory.page`
- `memory.fetch_context`
- `memory.list_bindings`
- `memory.provider_status`

Provider-specific capabilities may still exist, but they should not replace the canonical vocabulary.

### 4. Writable memory

Both the operator and the agent should be able to write memory through explicit tool or API surfaces.

This includes:

- registering a resource or artifact
- creating a semantic assertion or note
- requesting a provider-managed update

### 5. Search and exploration

Memory should be queryable without requiring the agent to know provider internals.

At minimum:

- search
- page or record inspection
- citation or grounding lookup
- context fetch for bounded turn attachment

### 6. Signal integration

Provider-originated events should enter Belltower through Signals and Reactions, not through hidden callbacks.

Examples:

- a Semaphora review task
- a Semaphora action signal
- a watched figure change
- a watched draft file change

### 7. Explicit context attachment

Memory should not inject itself into the model silently.

Belltower should only attach memory context through:

- explicit tool calls
- reaction-mediated setup before a turn
- future hooks under explicit policy

When a memory bundle becomes model-visible, the turn context manifest should
record an explicit attachment reference for it. The attachment record is the
bridge between memory provenance and the provider-call boundary; hidden prompt
text is not an acceptable substitute.

### 8. Scope awareness

Memory bindings should support meaningful scopes such as:

- global
- project
- workflow
- session-visible but not session-owned

The default model should not assume one isolated memory graph per session.

### 9. Inspectability

If memory affects work, the operator should be able to inspect:

- which provider supplied the memory
- which references or citations were attached
- which reaction caused the attachment
- which signal or user action caused the retrieval

### 10. Failure isolation

If a memory provider is unavailable, Belltower should still function.

Provider failure may block a memory operation, but must not break:

- session writes
- signal logging
- ordinary turns without memory

## What Belltower memory owns

Belltower memory should own:

- provider bindings and scope mapping
- provider-neutral memory tools and APIs
- registration of Belltower artifacts into provider memory spaces
- provider health and inspection surfaces
- context-bundle retrieval and attachment orchestration
- normalization of provider-originated events into Belltower signals
- Belltower-side routing state, work items, and replayable turn setup around memory

## What Belltower memory does not own

Belltower memory should not own:

- the provider's semantic database
- provider-native graph shape
- provider-native watch loops
- reaction policy
- hidden prompt injection
- the session's operational history

Semaphora should remain the semantic system.
Belltower memory should remain the harness-side integration layer.

## Data ownership and storage model

The ownership split should stay explicit.

### Provider-owned

The provider owns:

- semantic objects and indexes
- provider-native grounding model
- provider-native review tasks and change events
- native search and page assembly behavior

### Belltower-owned

Belltower owns:

- memory bindings
- provider auth or connection metadata
- mapping from Belltower scopes to providers
- canonical signals produced from provider events
- reaction outcomes and work items
- explicit references to memory objects used in a turn
- optional bounded caches of retrieved context bundles when needed for replay or performance

Belltower should store references and bounded attached context, not a duplicate semantic graph.

## Built-in working-memory provider

Belltower should start with a tiny built-in working-memory provider of its own.

This is useful because it lets the team validate:

- the provider contract
- the client/server surface
- the canonical tool vocabulary
- scope rules
- context-attachment mechanics
- inspection and debugging

The built-in provider should be:

- document-based
- small
- provider-neutral in behavior
- treated as if it were an external provider from the perspective of the memory subsystem

It should not become an excuse to skip the real provider boundary.

## Memory and signals

Memory-originated events should enter Belltower through the Signals subsystem.

That means:

1. the provider emits or exposes a memory event
2. a memory binding or adapter normalizes that event into a canonical Belltower signal
3. Belltower commits the signal
4. reactions decide whether to log, notify, create a work item, queue a turn, or start work

Examples:

- `semaphora.review_task.created`
- `semaphora.action_signal.created`
- `semaphora.change_event.created`

Memory events are therefore:

- signals first
- possible turn inputs later

They are not provider-owned prompt injections.

## Memory context attachment

Memory context should be attached conservatively.

The preferred order is:

1. explicit memory tool use by the agent or operator
2. reaction-mediated retrieval of a bounded context bundle
3. future hook-driven lifecycle attachment under explicit policy

The important rule is:

- if memory influences a turn, the attachment should be explicit and inspectable in Belltower's canonical event trail
- if memory is included in a provider call, the corresponding
  `turn.context_manifest.recorded` payload should include a model-visible
  attachment ref rather than relying on prompt-text reconstruction

Examples of acceptable turn attachment:

- a bounded Semaphora page bundle with citations
- a small set of review-task items related to the triggering figure change
- a built-in working-memory document snapshot

Examples of unacceptable attachment:

- a provider silently mutating the system prompt
- an unbounded dump of provider state with no citations

## Memory and hooks

Hooks are relevant to memory, but they are not the foundation.

Memory should first integrate through:

- tools
- signals
- reactions

Hooks become useful later for lifecycle seams such as:

- `on_session_start`
- `pre_llm_call`
- `post_tool_call`
- `pre_compact`
- `on_session_end`

Examples:

- fetch a small working-memory bundle at session start
- register a newly emitted figure after a tool call
- flush or summarize provider state before compaction

But a watched external file change from Semaphora is still a signal, not a hook.

See:

- [`./hooks-and-lifecycle.md`](./hooks-and-lifecycle.md)

## Canonical event implications

The canonical event taxonomy should eventually make memory integration visible.

Likely event families include:

- `memory.binding.registered`
- `memory.binding.updated`
- `memory.binding.disabled`
- `memory.resource.registered`
- `memory.context.requested`
- `memory.context.attached`
- `memory.tool.called`
- `signal.received` for memory-originated provider events

The exact taxonomy can evolve, but the rule should remain:

- memory influence on agent behavior must be canonical and inspectable, not inferred from rendered prompt text
