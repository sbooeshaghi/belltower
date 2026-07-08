# Belltower Developer Guidelines

This document defines the high-level development rules an AI agent should follow when writing or changing code in Belltower.

Read it with:

- [`../../AGENTS.md`](../../AGENTS.md)
- [`../architecture/overview.md`](../architecture/overview.md)
- [`../architecture/event-taxonomy-and-write-path.md`](../architecture/event-taxonomy-and-write-path.md)
- [`../architecture/tool-system-and-normalization.md`](../architecture/tool-system-and-normalization.md)
- [`../architecture/tool-policy.md`](../architecture/tool-policy.md)
- [`../architecture/memory.md`](../architecture/memory.md)
- [`../architecture/memory-subsystem-spec.md`](../architecture/memory-subsystem-spec.md)
- [`./source-of-truth-matrix.md`](./source-of-truth-matrix.md)
- [`./pattern-catalog.md`](./pattern-catalog.md)

This document is not a replacement for the architecture docs.
It is the development-facing rule set for how an agent should turn those architectural commitments into code.

## 1. Core Mental Model

Treat Belltower like a real autonomous system with cooperating subsystems, not like a monolithic chat app.

The right mental model is closer to an aircraft than a shell script:

- there is an event loop
- there are control surfaces
- there are operator interfaces
- there is telemetry
- there are subsystems that must be independently understandable and reliable

Belltower is an AI agent harness composed of:

- an LLM event loop
- context management
- tools
- canonical telemetry and replay
- higher-level features that emerge from those foundations
  - MCP
  - inspection
  - history
  - memory
  - signals and reactions
  - skills
- multiple operator clients and UIs

The right development stance is:

- strengthen the core seams
- let higher-level capabilities emerge from those seams
- avoid shortcuts that collapse subsystem boundaries for local convenience

## 2. Architectural First Principles

### Modularity

Every major concern should have a clear home.

Do not smear logic across crates or layers just because it is faster in the moment.

Before making a change, ask:

- which subsystem truly owns this?
- which subsystem should only consume it?
- which subsystem should only expose it?

If the answer is unclear, resolve the architecture first.

### Separation of concerns

Belltower should preserve clean boundaries between:

- agent loop
- runtime orchestration
- session storage
- protocol surfaces
- clients
- providers
- tools
- memory
- signals and reactions
- operator UI

Do not let one subsystem quietly absorb another subsystem's job.

Examples:

- `bt-agent` should not own storage or server behavior
- `bt-tui` should not become a control-plane owner
- `bt-server` should not become the semantic owner of memory or tools
- `bt-session` should not become a duplicate semantic database

### Maintainability

Prefer designs that remain legible after months of iteration.

That means:

- centralized ownership
- explicit contracts
- typed boundaries
- predictable data flow
- minimal special cases

A good local shortcut that creates future ambiguity is usually a bad Belltower change.

### Functional requirements first

When designing, updating, or changing a subsystem, identify its functional
requirements before deciding on the implementation.

This helps drive the development process from the real job the subsystem needs
to do instead of from:

- the current file layout
- the easiest local patch
- the existing tests
- a borrowed implementation from another system

Good subsystem work usually starts by clarifying:

- what the subsystem must do
- what it must not do
- what it owns
- what other subsystems need from it
- what invariants it must preserve
- how operators and agents interact with it

In Belltower, this is especially important because many subsystems are
cross-cutting:

- tools
- memory
- signals and reactions
- telemetry
- clients
- approvals
- context management

If the functional requirements are unclear, the implementation will usually end
up overfitted to the current code rather than aligned with the architecture.

### Practicality

Choose the simplest design that preserves the real subsystem boundaries.

Simple does not mean:

- hiding complexity in one large file
- pushing everything into one runtime object
- using shell commands because they are fast to add

Simple means:

- the design matches the real system shape
- the interfaces are small
- the behavior is easy to reason about
- the implementation can evolve without architectural debt

### Scalability

Assume Belltower will grow in:

- number of subsystems
- number of sessions
- number of providers
- number of operator surfaces
- number of derived capabilities

Do not design only for the current single-path use case if the architecture already says the system is broader.

### Data shapes are first-class

Data shapes are not incidental.
They determine how systems interact.

In Belltower, durable shapes such as:

- event envelopes
- DTOs
- tool payloads
- memory refs and context bundles
- signal envelopes
- approval objects

are core design surfaces.

If a data shape is weak, vague, or inconsistent, the system around it will also be weak.

### Shape changes are architecture changes

Changes to important shapes should be treated with the same care as API changes.

This includes:

- event taxonomies
- wire DTOs
- tool request and result payloads
- approval objects
- memory object views
- context-bundle shapes
- signal envelopes

If one of these shapes changes, assume that multiple layers may need to move:

- storage
- protocol
- runtime
- clients
- exports
- tests
- docs

Do not treat a shape change like a small local refactor if other subsystems depend on it.

### The harness is the product

Belltower's durable value is the harness itself:

- orchestration
- tools
- context management
- memory and signal seams
- telemetry
- replay
- evaluation
- operator control surfaces

Do not overfit Belltower around one model, one provider, one benchmark, or one
temporary workflow. Models will improve and provider APIs will change. The
harness should preserve a stable operational core that can use better models
without rewriting its control plane.

### Complexity must pay rent

Start from first principles and define the minimal components needed for the
real job.

Add complexity only when it buys at least one of:

- safety
- observability
- recoverability
- extensibility
- measurable performance
- operator leverage
- clearer subsystem ownership

Avoid abstractions that do not solve a measured problem or protect a known
seam. Belltower needs real structure, but every structure should justify itself
against real workloads, failure modes, and maintenance costs.

### Context is a budget

Treat model context as a constrained resource, not a dumping ground.

Prefer:

- high-quality instructions
- explicit context attachments
- durable provenance
- bounded retrieval
- precise tool outputs
- compaction with inspectable boundaries

Avoid:

- stale memory
- redundant instructions
- hidden prompt mutation
- excessive retrieval
- unbounded tool chatter
- prompt verbosity that cannot be justified

Context quality matters more than context volume. If a subsystem changes what
the model sees, that change should be reconstructable from canonical session
state.

### Observability enables automation

Automate only after the workflow is observable, evaluable, and recoverable.

For Belltower, this means the default order is:

1. define the state and tool shapes
2. record what happened durably
3. expose inspection and replay
4. add regression or acceptance checks
5. then automate the workflow

Autonomy without inspection is not leverage. It is hidden state.

### Recoverability beats brittle determinism

LLM systems are probabilistic. Belltower should not pretend otherwise.

Use:

- typed state
- explicit failure records
- validation
- bounded retries
- constrained execution
- approvals where needed
- pause, resume, steer, and replay

The goal is not perfect determinism. The goal is that failures are explainable,
recoverable, and auditable.

### Design for model evolution

Provider and model capabilities will change quickly.

Keep orchestration layers thin. Keep provider-specific quirks behind provider
adapters. Version public shapes carefully. Do not encode assumptions that only
hold for one current model's tool-calling, streaming, reasoning, or context
behavior unless the assumption is explicitly isolated and documented.

## 3. Source of Truth Rules

Follow [`./source-of-truth-matrix.md`](./source-of-truth-matrix.md).

In practice:

- architecture docs are normative for long-lived boundaries and invariants
- subsystem docs are normative for current structure
- code and focused tests are normative for current implemented behavior
- development docs explain how to work, not what is already implemented

When in doubt:

1. preserve explicit architectural invariants
2. fix code that violates them, or revise the architecture doc explicitly
3. do not silently code around ambiguity

### Refactor deliberately

Do not be afraid to:

- delete code
- move code
- rewrite code
- simplify code
- refactor a subsystem boundary

if doing so produces a better architectural result.

Preserving poor structure just because it already exists is not a Belltower virtue.

But refactoring should still be disciplined.
Before making a structural change, verify that the direction is actually desirable by checking:

- the architecture docs
- the relevant subsystem docs
- the current user-visible behavior
- neighboring crate boundaries
- the operational and telemetry implications

Important rule:

- code, tests, and docs may all be stale

That means an agent should not assume existing tests or existing implementation structure are automatically correct.
Sometimes the right change is to update the code.
Sometimes the right change is to update the tests.
Sometimes the right change is to update the docs.
Sometimes all three must move together.

What matters is not preserving the old shape.
What matters is restoring or improving coherence between:

- the architecture
- the implementation
- the tests
- the operator-facing behavior

Refactor boldly when the architecture calls for it, but do not refactor casually.
The change should make subsystem ownership, data flow, and future extension clearer than before.

## 4. Wiring Rules

Wiring matters.

Good wiring is one of the main ways Belltower stays modular.

### Wire through the intended client boundary

If there is a client boundary, use it.

Examples:

- TUI should talk through `bt-client`
- launcher should use the server/client path rather than hidden local shortcuts
- human-facing clients should consume protocol surfaces, not runtime internals

Do not bypass `bt-protocol` or `bt-client` for convenience.

### Keep runtime composition in runtime

`bt-runtime` is where subsystem composition belongs.

That means runtime may orchestrate:

- tool access
- approvals
- context attachment
- memory visibility
- signal routing

But it should not re-own the semantics of the subsystems it composes.

### Keep storage canonical

`bt-session` owns the canonical operational record.

If a feature needs durability, ask:

- is this canonical session truth?
- or is it provider-owned truth that Belltower should only reference?

Do not invent second stores of truth casually.

### Keep server thin in ownership

`bt-server` should expose canonical behavior, not quietly own cross-cutting semantics that belong in runtime, memory, or tools.

### Keep clients as consumers

Clients should render, inspect, and invoke.
They should not become shadow runtimes.

### Preserve inspectability and truthfulness

When the harness makes an important decision, the operator should be able to inspect:

- what happened
- why it happened
- which subsystem decided it
- which inputs or policy drove it

And when the harness does not know something, it should not pretend it does.

Truthful systems are easier to debug, easier to trust, and easier to extend.

This applies to:

- readiness
- approvals
- memory attachment
- tool availability
- provider support
- signal routing
- operator UI state

## 5. Design Around First-Class Data Shapes

When adding a feature, define the data shapes before spreading logic across the repo.

Typical order:

1. define the core domain object or envelope
2. define the storage or wire representation if needed
3. define the runtime/service interface
4. define the client-facing or tool-facing surface
5. only then implement the behavior

This is especially important for:

- event families
- tool request and result shapes
- memory object views and context bundles
- signal envelopes
- approval objects
- operator inspection surfaces

If the shape is wrong, patching consumers later will create churn and inconsistency.

### Define failure semantics early

Every meaningful subsystem should have explicit failure semantics.

Ask:

- how can this subsystem fail?
- what does that failure block?
- what must it never break?
- is degraded operation allowed?
- what should the operator see when it fails?

This is especially important for:

- providers
- auth
- memory
- signals and reactions
- context attachment
- inspection and export

Undefined failure behavior usually becomes inconsistent runtime behavior.

### Design bounded systems

Any subsystem that can grow, fan out, or accumulate state should define
boundedness rules early.

Examples:

- payload size limits
- context attachment budgets
- queue depth limits
- result limits
- retry limits
- search window limits
- compaction or snapshot budgets

Unbounded systems often look simpler at first and become harder to operate later.

## 6. Build From Stable Foundations

Belltower's durable foundation is:

- canonical event log
- canonical write path
- protocol boundary
- runtime composition
- tool system
- context management

Other features should be built as consequences of those foundations whenever possible.

Examples:

- inspection should reuse canonical session data
- memory use should appear through tool calls or explicit attachment
- signals should be canonical inputs before reactions act on them
- operator UIs should consume the same server-defined surfaces

When adding a feature, prefer:

- a clean extension of an existing seam

over:

- a one-off side channel

## 7. How To Add A New Capability

When adding a new capability, the default discipline is:

1. decide which subsystem owns it
2. identify which other subsystems only need references or projections
3. define the key data shapes
4. decide whether it is:
   - canonical session truth
   - provider-owned truth
   - derived/operator-only state
5. wire it through the intended boundaries
6. expose it through the smallest honest surface
7. document the ownership and public behavior
8. verify the touched path end to end

Do not start by editing a UI or server route and then retrofitting the architecture later.

### Prefer vertical slices over isolated abstractions

When possible, implement a thin end-to-end slice across the real subsystem boundaries before broadening a feature.

That usually means:

- one honest domain shape
- one runtime path
- one protocol or tool surface
- one operator inspection path
- one verification path

This is usually better than building a large local abstraction in isolation and hoping it wires in later.

Vertical slices keep the architecture honest.

### Respect migration work

When improving an existing subsystem shape, account for migration work explicitly.

That may include:

- stored-state migration
- protocol compatibility handling
- projection refresh changes
- fixture updates
- test rewrites
- doc rewrites

Do not treat migrations as an afterthought when changing canonical shapes or durable subsystem contracts.

## 8. Tools Are Harness Contracts

Treat tools as first-class harness surfaces, not just implementation helpers.

A tool should exist natively when:

- its semantic trace matters
- its policy differs from generic shell execution
- its input or output shape matters for replay, inspection, or future training
- it affects suspension, approvals, or durable state

Tool design should respect:

- [`../architecture/tool-system-and-normalization.md`](../architecture/tool-system-and-normalization.md)
- [`../architecture/tool-policy.md`](../architecture/tool-policy.md)

Do not add a tool without a policy story.

At minimum, every canonical tool should have an explicit position on:

- risk
- mutability
- approval behavior
- concurrency behavior
- interrupt behavior
- visibility and discoverability
- provenance or boundedness requirements if relevant

## 9. Telemetry Is Not Optional

Telemetry is part of the product.

Belltower's canonical session record should remain:

- durable
- inspectable
- replayable
- branch-aware
- exportable

That means:

- no canonical event before commit
- no client-local event truth
- no hidden bypass around the session store
- no feature-complete provider integration without raw stream durability where required

If a new subsystem materially affects agent behavior, ask how that effect becomes visible in the operational record.

### Keep a provenance and replay mindset

For any subsystem that materially affects agent behavior, ask:

- how is this recorded?
- how is it replayed?
- how is it inspected later?
- what references or bounded snapshots are needed?

This matters especially for:

- approvals
- memory
- signal-triggered work
- context attachment
- exports
- provider output normalization

If the effect cannot be reconstructed later, the subsystem is probably missing an important operational surface.

## 10. Memory, Signals, And Other Higher-Level Features

These subsystems should stay explicit and provider-neutral.

Examples:

- memory is a cross-session semantic subsystem, not prompt stuffing
- signals are canonical ingress, not synthetic local prompts
- reactions are policy, not producer behavior
- hooks are lifecycle callbacks, not a second orchestration plane

When working in these areas, preserve the same pattern:

- explicit contracts
- explicit ownership
- explicit operational logging
- explicit operator inspection

The more autonomous the system becomes, the more these boundaries matter.

## 11. Prefer Honest Simplicity

Thoughtful design is often the most simple in practice.

In Belltower, honest simplicity usually means:

- fewer special cases
- fewer hidden paths
- one source of truth per concern
- small interfaces
- explicit data flow
- explicit wiring

Dishonest simplicity looks like:

- “just call runtime directly from the client”
- “just store another copy here for convenience”
- “just infer this from rendered text”
- “just let the provider inject the prompt”

Avoid dishonest simplicity.

## 12. Clients Serve Operators

Belltower has, and will continue to have, multiple operator surfaces.

That means:

- UIs should consume protocol-defined behavior
- human inspection should stay aligned with agent-visible inspection where possible
- operator actions should flow through canonical runtime paths

If a client behavior needs information that no canonical surface provides, that is often a sign the underlying subsystem contract needs work.

## 13. Design For Autonomous And Human Use Together

The system should work well both:

- autonomously
- under direct human supervision

This means every subsystem should support both:

- machine-usable structured interfaces
- human-inspectable operational surfaces

The best Belltower designs make those two modes reinforce each other.

## 14. Git And Change Discipline

Belltower work should remain easy to review and easy to trust.

Follow these rules:

- keep changes scoped to one architectural slice
- stage only the files changed in that slice
- do not use broad staging commands
- inspect the worktree before committing
- do not revert unrelated user changes
- do not make destructive cleanup moves in a shared dirty tree

If the user asks for commits, commit deliberately and narrowly.

## 15. Documentation Discipline

If you change the architecture, update the architecture doc.
If you change the current structure, update the subsystem doc.
If you change the working process, update the development doc.

Do not leave future agents to infer the intended boundary from scattered code.

Good Belltower changes usually include:

- code changes
- tests where appropriate
- doc alignment where behavior or architecture changed

## 16. Final Heuristic

Before finishing a change, ask:

- does this strengthen or weaken subsystem ownership?
- does this preserve the canonical write path?
- does this improve or degrade operator inspectability?
- does this make the system easier or harder to extend later?
- would another agent understand where this logic belongs six months from now?

If the answer is unclear, the design probably needs one more pass.
