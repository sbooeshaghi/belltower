# Tool System and Normalization

This document defines the intended shape of Belltower's tool system.

Read it with:

- [`./claude-code-lessons-and-adoption.md`](./claude-code-lessons-and-adoption.md)
- [`./tool-policy.md`](./tool-policy.md)

It exists because tools are one of the harness boundaries that most strongly affect:

- model behavior
- trace quality
- approval quality
- compaction correctness
- future fine-tuning and replay

Belltower already has a working normalized tool system for the initial release:

- the canonical built-in names are now short and normalized
- self-inspection is now exposed to the model through a single `inspect` tool
- `plan` and `ask` now exist as native built-ins
- built-ins are now gated by session tool mode
- there is no model-facing alias layer
- richer tool metadata now exists, but runtime use of it is still early
- the native-tool vs CLI vs implicit-context split still needs continued discipline

This document defines the durable direction on top of that current state.

Backward compatibility is not a design constraint here.

## Current Belltower State

Today Belltower exposes:

- built-in tools:
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
- conditional built-in meta-tool:
  - `catalogue`
- human-facing inspection still exists through slash commands and protocol routes
- MCP tools using server-defined MCP names

This is now the clean canonical vocabulary Belltower should prefer in traces, exports, and future fine-tuning data.

The current implementation also has a typed metadata layer on canonical tools:

- `risk_class`
- `is_read_only`
- `is_concurrency_safe`
- `interrupt_behavior`
- `should_defer`
- `catalogue_tags`
- `display_group`

That metadata is now consumed by approval policy and deferred-tool discovery.
The canonical event model also includes `tool.operation.recorded`, an additive
operation envelope that can carry initiator, policy classification, and artifact
references without changing the tool-call argument/result events. This keeps
agent tools, human-invoked tool operations, and MCP tools on one telemetry seam
while preserving open-ended tool names.

For the broader execution contract around visibility, approvals, boundedness,
concurrency, and interruption, see:

- [`./tool-policy.md`](./tool-policy.md)

## Design Goals

The tool system should be:

- simple enough for models to use reliably
- rich enough that traces capture intent rather than hiding it in shell strings
- explicit about risk and approval boundaries
- stable enough to become the preferred vocabulary for future Belltower-trained models
- flexible enough to coexist with MCP and provider-specific behavior

The key rule is:

- Belltower's canonical tool vocabulary belongs to Belltower, not to any model vendor

## Core Principles

### 1. Canonical Tool Names Belong To The Event Log

Belltower should store canonical tool names in the session/event store.

Those canonical names should be:

- short
- lowercase
- unambiguous in context
- stable enough to reuse across exports and training

The event store should keep tool names as strings, not a closed Rust enum.

That matters because:

- MCP tools are inherently open-ended
- the canonical built-in set will still evolve
- imported traces may need normalization into canonical strings

So the stable commitment is:

- Belltower documents a preferred canonical built-in vocabulary
- the store records canonical strings
- operation metadata is stored as a separate durable envelope rather than being
  smuggled into tool arguments
- adding a new built-in tool should not require a storage-schema migration just because the name is new

## Current Implementation Notes

The current code now includes the first Claude-Code-inspired slice:

- canonical short built-in names
- typed tool metadata in `bt-core`
- runtime approval policy driven by approval requirement plus tool metadata
- conditional `catalogue` registration
- exact session-history search through the provider-compatible `session_search`
  built-in
- deferred MCP tools hidden from the model-facing prompt once `catalogue` is present

What is still not implemented:

- model-family alias tables
- richer runtime scheduling that uses `is_concurrency_safe` and
  `interrupt_behavior`
- a stronger native/CLI/implicit split for lower-value capabilities

### 2. Native Tools Are For Semantic Operations

Native tools should exist when the structured trace matters.

That includes cases where:

- the operation is semantically distinct from generic shell execution
- approval policy should differ from `shell`
- the operation should survive compaction as a clear intent
- the operation should produce training signal that would be lost inside bash
- the operation needs turn suspension or resume semantics

This is why Belltower should not follow Codex's "use shell for almost everything" approach as its own canonical model.

That pattern works for Codex because Codex was trained on it.
For Belltower, it would weaken the telemetry moat.

### 3. Not Every Capability Should Be A Native Tool

Belltower should intentionally support three patterns:

- native tools
- CLI capabilities via `shell`
- implicit injected context

The split should be:

#### Native tools

Use for:

- codebase operations
- planning
- fetch
- human interaction
- self-inspection
- later workflow and memory actions

#### CLI via `shell`

Use for:

- operational commands
- diagnostics
- low-frequency session-management commands
- things whose structured trace value is not yet high enough to justify a native tool

Examples:

- `bt status`
- `bt history`
- `bt export`
- `bt diff`

#### Implicit injection

Use for:

- information that should almost always be available without a call
- active plan state
- startup memory hints
- short readiness or session context hints

The native-tool boundary should stay intentional rather than maximalist.

### 4. MCP Tools Stay External

MCP tools should not be renamed into Belltower's canonical vocabulary.

They remain:

- server-defined names
- separately discoverable capabilities
- first-class participants in approvals, telemetry, and session traces

Belltower's canonical built-in tool vocabulary and the MCP namespace should sit alongside each other, not collapse into one naming scheme.

### 5. Hooks Are A Separate Runtime Layer

Belltower should treat hooks as a future lifecycle-extension mechanism, not as a synonym for tools or skills.

See:

- [`./hooks-and-lifecycle.md`](./hooks-and-lifecycle.md)

The split should stay explicit:

- tools are model-callable capabilities with schemas, approvals, and traceable results
- skills are reusable procedural guidance and supporting materials
- hooks are runtime callbacks that fire at stable lifecycle points whether or not the model explicitly asked for them

That makes hooks relevant to tool architecture, but on a different axis.

The likely future hook points are:

- `on_session_start`
- `on_session_end`
- `pre_llm_call`
- `post_llm_call`
- `pre_tool_call`
- `post_tool_call`
- `on_error`

Hooks belong in the runtime or future plugin layer, not in `bt-agent`.

They should be:

- triggered from stable runtime lifecycle boundaries
- backed by canonical events wherever the behavior matters for telemetry or replay
- useful for policy, provenance, redaction, memory flush, metrics, and later plugin extensions

They should not be used to replace:

- canonical tool definitions
- approval policy
- prompt assets
- skills
- the canonical event log

This is intentionally a later extension point rather than part of the initial release surface.

## Recommended Canonical Built-In Vocabulary

The recommended canonical vocabulary is split into:

- near-term built-ins Belltower should actively converge on
- later built-ins that belong in the architecture but should not be treated as immediate implementation requirements

### Near-Term Canonical Built-Ins

These are the tools Belltower should actively normalize toward.

#### Codebase

- `read`
- `write`
- `edit`
- `shell`
- `search`
- `list`

#### Planning And Interaction

- `plan`
- `ask`
- `inspect`

#### Web Retrieval

- `web_search`
- `web_fetch`

This gives Belltower a clean eleven-tool built-in vocabulary that is large enough to carry intent and small enough to stay practical.

### Later Canonical Built-Ins

These belong in the longer-term architecture, but should not block the near-term cleanup:

- `branch`
- `delegate`
- `undo`

These are good canonical names, but they depend on stronger underlying runtime semantics:

- `branch` depends on native agent-facing branch control
- `delegate` depends on child-session execution policy and result contracts
- `undo` depends on strong mutation provenance and reversible file-operation history

So they should be treated as planned canonical tools, not immediate built-in requirements.

The operator-facing `/spawn [--connection <id>] [--model <model-id>] <objective>` command exercises the child-session lineage path today, but it is intentionally not a replacement for a model-facing `delegate` tool. A future `delegate` tool must use the same child-session semantics while also defining worktree isolation, depth limits, budget behavior, approval inheritance, and a typed return artifact.

Cross-session memory should instead grow behind the explicit `memory.*` tool family.
If Belltower later introduces `recall`, it should be only a convenience wrapper over
that subsystem rather than a foundational canonical built-in.

### Conditional Meta-Tool

`catalogue` is useful, but it should not be part of the core built-in set by default.

It should be registered only when:

- the available MCP/custom tool count is large enough that prompt cost becomes material
- and tool discovery is clearly better than listing all tools directly

So the recommendation is:

- `catalogue` is conditional and optional
- not part of the core canonical built-in vocabulary

## Recommended Built-In Shapes

### `read`

Purpose:

- read a known file
- optionally by line range

This is the canonical built-in name.

### `write`

Purpose:

- create or fully overwrite a file

This is the canonical built-in name.

Important semantic rule:

- `write` means "I know the whole desired file content"
- it is distinct from `edit`

### `edit`

Purpose:

- make a surgical change to an existing file

This is the canonical built-in name.

The exact editing payload can evolve, but the semantic distinction must remain:

- `write` is whole-file intent
- `edit` is localized modification intent

### `shell`

Purpose:

- general escape hatch

This remains necessary and high-risk.
It should stay the broadest tool and the most approval-sensitive one.

### `search`

Purpose:

- find content or files

This is the canonical built-in name, and it supports:

- `mode = content`
- `mode = path`

That is better than separate `grep`/`glob`-style built-ins for Belltower because it keeps the built-in count low while preserving intent in structured arguments.

Note:

- the current built-in `search` is repository-oriented, not a full external web-search capability
- external web research must use `web_search` plus `web_fetch`, not overload repository `search`

### `list`

Purpose:

- enumerate directory structure

This is the canonical built-in name.

### `web_search`

Purpose:

- discover external public web sources for a query
- return ranked source-attributed results without fetching every page body

This is the canonical web-discovery built-in. It keeps external
retrieval explicit in traces and avoids hiding provider-native web
search behind model behavior.

Current policy direction:

- `web_search` is approval-sensitive until trust/domain policy exists
- backends are selectable behind the same tool shape; Exa is the first
  supported backend, configured through `[web]` plus the auth-store/env
  credential resolver
- adding a provider must not change the model-facing tool contract

Operators configure the default search backend separately from the
tool schema:

```bash
belltower web status
belltower web configure exa
belltower web login exa
```

Stored credentials use provider ids such as `web:exa`. Environment
sources such as `EXA_API_KEY` remain supported, but backend readiness
and default selection are inspectable through Belltower rather than
being hidden shell state.

### `web_fetch`

Purpose:

- retrieve public URL content with structured status, provenance, body,
  truncation, and digest metadata

This is the canonical built-in name.

Current policy direction:

- `web_fetch` should not be treated as universally safe
- it should be governed by explicit operator approval until richer domain/trust policy exists

Note:

- `web_fetch` is URL-oriented; it should not search
- richer extraction can grow behind `web_fetch` formats or a future
  `web_extract` tool only if real workloads prove that separate
  capability is worth the extra model-facing surface

### `plan`

Purpose:

- create, update, complete, and inspect active plan items

This should be a native built-in because:

- it survives compaction better than prose planning
- it produces clean trace signal
- it supports human and agent inspection naturally

### `ask`

Purpose:

- request explicit human input through the normal tool protocol

This should be a native built-in because:

- it needs turn suspension and resume semantics
- it should be distinguishable from approvals
- it creates explicit human-in-the-loop trace points

### `inspect`

Purpose:

- structured self-inspection against the canonical store

This is the broad canonical model-facing inspection tool, replacing the earlier
`inspect_*` family. Narrow, high-value inspection operations can still have
their own tool when the separate trace semantics are useful, as with
`session_search`.

Recommended shape:

- `inspect({ query: "session" })`
- `inspect({ query: "queue" })`
- `inspect({ query: "errors", limit: 10 })`
- `inspect({ query: "plan" })`
- `inspect({ query: "turns", limit: 10 })`
- `inspect({ query: "execution", limit: 10 })`
- `inspect({ query: "raw_turn", turn_id: "...", limit: 20 })`
- `inspect({ query: "lineage" })`
- `inspect({ query: "workflow" })`
- later:
  - `inspect({ query: "files_modified" })`
  - `inspect({ query: "token_usage" })`

This keeps the runtime inspection surface broad while keeping the model-facing tool count under control.

### `session_search`

Purpose:

- exact text search across canonical session history and telemetry

The concept is `session.search`, but the provider-facing tool name is
`session_search` so it remains compatible with model tool-name restrictions.
It is a read-only inspection tool, not memory, semantic retrieval, or hidden
prompt injection.

Default behavior searches the current branch lineage. Callers may explicitly
request all branches when they need cross-branch provenance. Results return
stable canonical references such as `session_id`, `branch_id`, `turn_id`,
`seq_id`, `event_id`, and `tool_call_id` where applicable so the model or a
human operator can follow up through normal inspection surfaces.

This should stay separate from `memory.search`. Session search answers "what
happened in this durable session trace"; memory search, when it exists, should
answer "what cross-session knowledge has been intentionally retained."

## Approval Model By Tool Family

The intended broad risk classes are:

### Normally safe

- `read`
- `list`
- `search`
- `plan`
- `inspect`
- later `memory.search`
- later `memory.view`

### Conditionally safe

- `web_search`
- `web_fetch`

This should respect trust policy, allowlists, or explicit operator settings.

### Medium risk

- `write`
- `edit`
- later `undo`
- later `branch`

### High risk

- `shell`
- later `delegate`

### Turn-suspending

- `ask`

This classification is more important than any one tool name.
Belltower should preserve approval semantics even as the built-in vocabulary evolves.

## Model-Specific Aliasing

Belltower should support model-specific aliasing eventually, but it should not block the current tool cleanup.

That means:

- the canonical vocabulary should be chosen first
- the store should always record canonical names
- a future alias layer may map model-facing names to canonical names in `bt-runtime`

But the current recommendation is:

- do not build full schema-transforming alias tables yet
- first normalize Belltower's own built-in vocabulary
- then test whether specific model families materially benefit from aliased names

This is a deliberate deviation from a more aggressive alias-first strategy.

Why:

- Belltower does not yet have enough empirical evidence that aliasing complexity is worth it
- single-word canonical names are already close to what smaller modern harnesses use
- runtime aliasing is easier to add later than to remove after it spreads through traces and docs

So the intended sequence is:

1. normalize the canonical Belltower tool vocabulary
2. add `plan` / `ask` / unified `inspect`
3. gate the exposed built-ins by session mode
4. only then consider model-family alias tables if the evidence justifies them

## Session Modes

Belltower should gate built-ins by mode to control prompt cost.

Recommended initial shape:

### Standard

- `read`
- `write`
- `edit`
- `shell`
- `search`
- `list`

### Extended

Adds:

- `plan`
- `web_search`
- `web_fetch`
- `ask`
- `inspect`

Later, once Belltower has distinct autonomous runtime semantics, it can add a separate autonomous mode that widens the exposed tool surface with capabilities such as:

- `branch`
- `delegate`
- `undo`
- `memory.*` tools when the underlying subsystem is present

That future mode should not be exposed until it means something distinct in runtime behavior or tool inventory.

## Remaining Alignment Work

The main normalization work is already done:

- built-ins use the canonical short names
- `inspect` replaced the earlier model-facing inspection family
- `plan` and `ask` are native built-ins
- session-mode gating exists
- `catalogue` exists as a conditional meta-tool

What remains is narrower:

1. deepen metadata-driven runtime behavior beyond approvals and deferred discovery
2. keep the native/CLI/implicit boundary explicit as more operator capabilities are added
3. leave model-family aliasing deferred until there is real evidence it improves external models materially
4. add later canonical tools such as `branch`, `delegate`, and `undo` only when the underlying runtime semantics are ready
5. keep cross-session memory behind the explicit `memory.*` family unless a later convenience wrapper proves necessary

## Crate Ownership

The intended ownership remains:

- `bt-tools`
  - canonical built-in definitions
  - guardrails
  - built-in schemas and executors
- `bt-runtime`
  - later session-mode gating
  - later model-facing alias resolution if adopted
  - runtime ownership of plan/memory/workflow semantics behind tools
- `bt-agent`
  - pure consumer of the model-facing tool list it is given
  - no awareness of aliasing or storage
- `bt-session`
  - canonical storage of tool-call events and tool results using canonical tool names

## Immediate Recommendation

The right near-term Belltower tool plan now is:

1. keep the current canonical built-in vocabulary stable
2. deepen metadata-driven runtime behavior and approval richness
3. continue to treat MCP as external and discoverable rather than aliasing it into canonical names
4. defer full model-specific aliasing until there is empirical evidence it is needed
5. add later canonical tools only when the underlying runtime semantics are real

This keeps the tool system:

- simpler than Codex
- more structured than shell-first harnesses
- more coherent than the earlier transitional Belltower state
- and aligned with Belltower's real moat: canonical, inspectable, replayable traces
