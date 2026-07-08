# Hooks and Lifecycle

This document defines the intended shape of Belltower's future hook system.

Read it with:

- [`./overview.md`](./overview.md)
- [`./tool-system-and-normalization.md`](./tool-system-and-normalization.md)
- [`./signals-and-ingress.md`](./signals-and-ingress.md)
- [`./reactions-and-playbooks.md`](./reactions-and-playbooks.md)
- [`./memory.md`](./memory.md)
- [`./skills-and-self-improvement.md`](./skills-and-self-improvement.md)

It exists because Belltower will eventually need stable runtime extension points around lifecycle boundaries without collapsing those concerns into:

- tools
- signals
- skills
- hidden agent callbacks

The central architectural rule is:

- hooks are deterministic lifecycle callbacks around runtime boundaries
- hooks are not external ingress
- hooks are not a substitute for tools, skills, signals, or the canonical event log

## Summary

Belltower should eventually have a first-class Hooks subsystem or hook layer.

That subsystem is responsible for:

- exposing stable lifecycle boundaries to trusted extensions
- allowing deterministic observation, gating, enrichment, and enqueueing around those boundaries
- preserving inspectability when hook behavior materially affects execution
- keeping lifecycle logic out of `bt-agent`

Hooks should be treated as a later extension layer, not the first implementation requirement for signals, reactions, or memory.

## Why hooks should stay separate

Without an explicit hook model, lifecycle behavior tends to get scattered into:

- provider-specific ad hoc callbacks
- special cases inside the agent loop
- synthetic tools that are not really model-invoked capabilities

That makes the harness harder to reason about.

The separation should stay explicit:

- tools are model-callable capabilities with schemas and approvals
- signals are durable ingress from outside the loop
- reactions are registered policies over committed signals
- hooks are deterministic callbacks at stable runtime lifecycle points

## What hooks are for

Hooks are useful for:

- policy checks around tool execution
- adding small bounded context at lifecycle boundaries
- artifact registration after deterministic harness events
- pre-compaction extraction or flushing
- metrics, provenance, redaction, and audit support
- initiating review workflows at stable boundaries

Hooks are not for:

- replacing the signal system
- letting producers inject hidden policy
- silently mutating prompts or provider state in ways Belltower cannot explain
- bypassing approvals or the canonical event log

## Lifecycle points

The likely future hook points are:

- `on_session_start`
- `on_session_end`
- `pre_llm_call`
- `post_llm_call`
- `pre_tool_call`
- `post_tool_call`
- `pre_compact`
- `on_error`
- future `on_subagent_start`
- future `on_subagent_stop`

The contract should stay small and typed.

## Allowed behaviors

Hooks should be allowed to:

- observe lifecycle input
- add bounded additional context
- request that a tool call be denied or routed to approval
- enqueue work for later processing
- emit metadata or side-channel audit information
- initiate review workflows

Hooks should not be allowed to:

- fabricate canonical events outside the normal write path
- silently rewrite the session transcript
- bypass approval policy
- mutate skills or memory state without explicit reviewable surfaces
- become a second orchestration plane parallel to Signals and Reactions

## Relationship to signals

Signals and hooks solve different problems.

Signals are for:

- external producers
- replayable ingress
- durable routing and policy

Hooks are for:

- internal runtime boundaries
- deterministic lifecycle interception

Examples:

- a Semaphora watched-file change is a signal
- a webhook callback is a signal
- a Slack message is a signal
- a `pre_tool_call` policy check is a hook
- a `pre_compact` extraction pass is a hook

Hooks may emit follow-up metadata or enqueue work, but they should not replace canonical signal ingress.

## Relationship to reactions

Hooks are not reactions.

The split should remain:

- hooks fire because the runtime crossed a stable boundary
- reactions fire because a committed signal matched a policy object

Hooks may trigger or enqueue later work, but they should not absorb the responsibilities of:

- signal matching
- work routing
- playbook packaging

## Relationship to memory

Hooks are relevant to memory integration, but only after the memory and signals boundaries are clean.

Useful memory-related hook examples include:

- `on_session_start`
  - load a small working-memory snapshot
- `pre_llm_call`
  - attach a bounded memory context bundle selected by policy
- `post_tool_call`
  - register a newly created figure or draft artifact with the memory subsystem
- `pre_compact`
  - ask a provider or built-in working memory to preserve useful state before context compression
- `on_session_end`
  - run a review workflow or flush working-memory changes

The important rule is:

- external memory updates remain signals
- hook-driven memory behavior remains lifecycle-local and policy-bounded

## Relationship to tools and skills

Hooks, tools, and skills should compose without collapsing into one another.

- a tool is something the model chose to call
- a skill is reusable procedural guidance
- a hook is a runtime callback that may run whether or not the model explicitly asked for it

This matters because a hidden callback should not be mistaken for:

- a model choice
- a user instruction
- a reusable skill

## Safety and implementation rules

The hook layer should obey strict rules.

### 1. Stable typed inputs

Hook inputs should be typed and versioned.

### 2. Bounded execution

Hooks should have:

- timeouts
- failure isolation
- explicit ordering rules

### 3. Explicit side effects

If a hook materially affects execution, that effect should be visible through canonical events or inspectable metadata.

### 4. No hidden prompt mutation

Additional context added by hooks should remain bounded and attributable.

### 5. Runtime ownership

Hooks belong in the runtime or a future plugin layer, not in `bt-agent`.

## Why hooks are not a prerequisite

Belltower does not need a hook system before it can build:

- Signals
- Reactions
- Playbooks
- Memory bindings

Those systems can and should be designed first.

Hooks are a later seam that improves modularity around lifecycle boundaries.

That means:

- define the hook boundary now
- avoid baking hook-like special cases into the runtime
- but do not block core subsystem design on hook implementation

## Canonical event implications

Where hook behavior matters for telemetry, replay, or debugging, Belltower should eventually record it.

Likely event families include:

- `hook.invoked`
- `hook.completed`
- `hook.failed`
- `hook.blocked`
- `hook.context.attached`

The exact taxonomy can evolve, but the rule should remain:

- if a hook changed what work was allowed to happen or what context the model saw, the operator should be able to inspect that fact

