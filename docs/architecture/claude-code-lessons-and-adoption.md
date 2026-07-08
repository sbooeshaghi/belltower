# Claude Code Lessons and Adoption

This note records what Belltower should learn from the local Claude Code review
without eroding the principles that make Belltower worth building.

It exists because Claude Code is currently the strongest product reference in the
landscape for:

- tool-system maturity
- permission and approval semantics
- operator auth/model UX
- bridge and remote-control productization
- long-running session ergonomics

But it is not Belltower's architectural template.

Belltower should copy Claude Code's strongest product ideas while explicitly
rejecting the parts of Claude Code that would weaken:

- session-as-telemetry
- canonical event ownership
- raw provider durability
- server-first protocol honesty
- provider-neutral core types
- crate-boundary clarity

## Source Discipline

The Claude Code review used two different source layers:

- the preserved original source on `origin/backup`
- the later `main` branch docs and explorer materials

Those later docs are useful orientation aids, but they are not primary design
documents from Anthropic.

The practical rule for Belltower is:

- use Claude Code source as the primary evidence
- use the later markdown as commentary and subsystem maps
- do not copy a design choice just because the explorer docs describe it cleanly

## Architectural Conclusion

Claude Code is best understood as a mature product monolith.

Its strongest qualities come from:

- a very rich tool interface
- a very mature permission system
- strong operator surfaces for model, auth, status, and bridge workflows
- serious support for delegated work, background tasks, and remote control

Its main structural weakness, from Belltower's perspective, is that too much of
the product is built on a transcript-first substrate.

That means Belltower should adopt Claude Code's:

- semantics
- UX quality
- policy vocabulary
- runtime ergonomics

while not adopting its:

- transcript-as-canonical-state model
- Anthropic-shaped core abstractions
- large fused product monolith

## Belltower Principles That Must Not Change

Nothing borrowed from Claude Code should violate these:

1. the canonical store is the source of truth
2. canonical events are committed before SSE and tracing fanout
3. raw provider chunks remain durable and queryable
4. `bt-agent` stays pure and reusable
5. provider output stays normalized into Belltower-owned core types
6. `bt-tui` remains a protocol client rather than an embedded runtime shell

If a Claude Code pattern conflicts with those rules, Belltower should not adopt
it.

## What Belltower Should Adopt

### 1. Rich Tool Metadata

Claude Code's biggest tool-system lesson is not its exact tool names.
It is that tools need more semantics than:

- a name
- a schema
- an execute function

Belltower should add an explicit tool-metadata layer that covers:

- `risk_class`
- `is_read_only`
- `is_concurrency_safe`
- `interrupt_behavior`
- `should_defer`
- `catalogue_tags`
- `display_group`

These fields should help:

- runtime scheduling
- approval policy
- tool discovery
- transcript rendering
- session-mode gating

But the separation of concerns should stay cleaner than Claude Code.

The key rule is:

- tool execution, tool metadata, and tool presentation should be related but not
  collapsed into one giant trait

### 2. A Stronger Permission Engine

Claude Code's permission system is one of the strongest things in the landscape.
Belltower should adopt the seriousness of that subsystem.

The next version of Belltower's permission model should support:

- layered rule sources
- explicit ask / allow / deny outcomes
- richer reasoning about why a decision was made
- per-tool policy behavior
- batch and plan-mode approval semantics
- first-class remote or delegated approval routing later

The approval engine should remain runtime-owned and event-backed.
It should not live inside the TUI or inside tool implementations.

### 3. Deferred Tool Discovery

Claude Code is right that large tool surfaces eventually need discovery.

Belltower should keep the canonical built-in set small, but when MCP and custom
tool counts become large, it should add:

- a conditional `catalogue` tool
- searchable metadata over available tools
- runtime-side filtering and ranking

That gives Belltower the same product advantage without letting tool count bloat
the core prompt.

### 4. Better Command Taxonomy

Claude Code's command layer is more mature than its public reputation suggests.

Belltower should adopt the idea that slash commands fall into distinct operator
classes:

- commands that invoke the model
- commands that run locally
- commands that return interactive UI state

That distinction is useful for:

- TUI rendering
- progress handling
- permission expectations
- future GUI parity

Belltower should keep its command layer thinner and more protocol-honest, but
the command taxonomy itself is worth copying.

### 5. Operator-First Auth and Model UX

Claude Code gets one important product truth right:

- login
- model selection
- status
- doctor
- bridge status

all feel like one coherent system.

Belltower should preserve its cleaner provider/connection split, but operator
flows should move closer to that bar:

- one obvious guided selector path
- truthful readiness display
- tight integration between status, doctor, auth, and model surfaces
- consistent TUI and launcher behavior

### 6. Background Work and Detach Semantics

Claude Code treats long-running and delegated work as a first-class operational
problem.

Belltower should adopt that seriousness in a way that fits its session graph:

- explicit detach semantics
- explicit session/background status
- stable child-session lifecycle inspection
- clear cancel vs leave-running behavior

This should be done through canonical events and workflow state, not hidden UI
state.

### 7. Optional Bridge-Style Subsystems

Claude Code shows that IDE bridge and remote-control features can become
important product surfaces.

Belltower should not build them early or broadly, but when it does, it should
follow these lessons:

- feature-gate optional subsystems cleanly
- keep them protocol-honest
- keep bridge state explicit and inspectable
- never let optional bridge logic distort the core runtime model

## What Belltower Should Explicitly Not Adopt

### 1. Transcript-First Canonical State

Belltower should not use a human-facing transcript as the primary store for:

- resume
- branching
- compaction
- delegation
- telemetry

Claude Code's transcript and session-history complexity is exactly the kind of
burden Belltower is trying to avoid.

### 2. Provider-Shaped Core Types

Belltower should not make Anthropic, OpenAI, or any other provider's block
shapes become the core domain model.

Provider adapters should map provider-native payloads into Belltower-owned:

- event kinds
- completion deltas
- message parts

with raw chunks preserving the provider-native evidence separately.

### 3. Fused Tool / UI / Permission Objects

Claude Code's tool objects are powerful, but they also fuse too many concerns.

Belltower should not define one giant abstraction that owns:

- execution
- schema
- approval logic
- UI rendering
- prompt text
- discovery metadata

The design should stay factored even as metadata gets richer.

### 4. Feature-Flag Sprawl As Core Architecture

Claude Code uses heavy build-time feature gating.

Belltower should stay more disciplined:

- a small number of feature gates
- clear runtime capability reporting
- minimal conditional compilation at the architectural boundary

### 5. Product Monolith Drift

The more Belltower learns from Claude Code, the more it risks re-creating the
same "everything in one product layer" shape.

The protection against that is to keep crate ownership explicit.

## Recommended Belltower Ownership Model

### `bt-tools`

Owns:

- canonical tool executors
- tool schemas
- tool metadata definitions
- tool result shapes

Does not own:

- final approval decisions
- TUI rendering
- provider/model aliasing

### `bt-runtime`

Owns:

- approval engine
- tool scheduling
- concurrency policy
- interrupt behavior handling
- session-mode tool gating
- conditional tool discovery and catalogue registration

This is the correct place for the Claude-Code-inspired policy layer.

### `bt-tui`

Owns:

- local presentation choices
- operator command UX
- approval interaction surfaces
- visibility toggles and transcript rendering

It should consume runtime/protocol truth, not redefine it.

### `bt-protocol`

Owns:

- command DTOs
- approval DTOs
- tool-result and status envelopes where they are user-facing protocol surfaces

### `bt-session`

Owns:

- canonical persistence of tool calls, tool results, approvals, and workflow
  events
- projection-backed read paths for tool and workflow inspection

## Recommended Implementation Order

The right sequence is:

1. add `ToolMetadata` and related runtime policy hooks
2. redesign the approval engine around richer rule sources and outcomes
3. add deferred tool discovery and `catalogue` gating
4. improve command taxonomy and command-state rendering
5. continue auth/model/status/doctor unification
6. later, apply the same discipline to bridge- or remote-style subsystems

This ordering copies Claude Code's strongest leverage points first while keeping
Belltower aligned with its own core thesis.

## Standard-Harness Test

This note should be considered successful only if the result is:

- more operator-trustworthy than today's Belltower
- more inspectable than Claude Code
- more architecturally coherent than Claude Code
- still clearly Belltower rather than a Rust port of Claude Code
