# Harness Landscape Review

Date: 2026-04-08

This document broadens Belltower's earlier case-study work into a wider survey
of contemporary agent harnesses under `/tmp/belltower-case-studies/harness`.

This is a dated reference audit.
It informed the release-hardening stack, but it is not the source of truth for
current implemented behavior after that stack.
Use [`./current-sprint.md`](./current-sprint.md) for the active release gate and
[`../development/source-of-truth-matrix.md`](../development/source-of-truth-matrix.md)
for conflict resolution.

It complements, but does not replace:

- [`./case-study-review.md`](./case-study-review.md)
- [`./target-implementation-goals.md`](./target-implementation-goals.md)
- [`../architecture/overview.md`](../architecture/overview.md)
- [`../development/developer-guidelines.md`](../development/developer-guidelines.md)

This is an architecture-first survey.
It is intended to answer:

- which subsystem seams recur across the stronger harnesses
- which architectural moves appear to scale well
- which product patterns are useful for Belltower
- which tradeoffs or anti-patterns Belltower should avoid

This is not a deep behavior audit of every codepath in every repo.
The emphasis is on:

- top-level docs
- workspace/package layout
- major entrypoints
- explicit subsystem docs when present

## 1. Scope

The survey covered these harness families:

- Codex and Every Code
  - `/tmp/belltower-case-studies/harness/codex`
  - `/tmp/belltower-case-studies/harness/code`
- OpenCode
  - `/tmp/belltower-case-studies/harness/opencode`
- pi-mono and oh-my-pi
  - `/tmp/belltower-case-studies/harness/pi-mono`
  - `/tmp/belltower-case-studies/harness/oh-my-pi`
- Goose
  - `/tmp/belltower-case-studies/harness/goose`
- Gemini CLI, Qwen Code, and Grok CLI
  - `/tmp/belltower-case-studies/harness/gemini-cli`
  - `/tmp/belltower-case-studies/harness/qwen-code`
  - `/tmp/belltower-case-studies/harness/grok-cli`
- Hermes Agent and Claude Code
  - `/tmp/belltower-case-studies/harness/hermes-agent`
  - `/tmp/belltower-case-studies/harness/claude-code`
- Aider
  - `/tmp/belltower-case-studies/harness/aider`

Lighter outlier references were also inventoried under:

- `/tmp/belltower-case-studies/harness/Warp`
- `/tmp/belltower-case-studies/harness/agent-of-empires`
- `/tmp/belltower-case-studies/harness/mistral-vibe`
- `/tmp/belltower-case-studies/harness/awesome-agent-harness`

## 2. Recurring Seams

Across the stronger harnesses, the recurring subsystem map looks like this:

### 2.1 Transport and control plane

The strongest harnesses do not treat transport as a convenience layer.
They expose a real control plane with its own types, APIs, or protocol
primitives.

Common examples:

- app-server and protocol crates in Codex
- ACP and sync/control-plane surfaces in OpenCode
- RPC or editor-integration surfaces in pi and Hermes
- headless or remote-control surfaces in Grok

Implication for Belltower:

- the server-first `bt-protocol` and `bt-client` direction is correct
- future clients should continue to sit on top of the same transport boundary
- remote control, workflow inspection, and editor integration should not bypass
  it

### 2.2 Core runtime and agent loop

The durable pattern is a reusable agent/runtime core beneath multiple shells:

- Codex: separate core, exec, TUI, CLI, protocol, app-server
- pi: separate provider API, agent core, coding agent, TUI, web UI
- Goose: separate crates for core, CLI, server, SDK, and MCP
- Gemini and Qwen: separate CLI from a shared core package

Implication for Belltower:

- `bt-agent`, `bt-runtime`, `bt-server`, `bt-client`, and `bt-tui` should
  remain distinct ownership boundaries
- new operator surfaces should consume those seams rather than collapsing them

### 2.3 Auth, config, and provider routing

Nearly every serious harness isolates auth and config rather than letting them
leak into the loop.

This shows up as:

- Codex account, config, keyring, and secrets crates
- Gemini and Qwen core packages carrying explicit auth/config/provider services
- Hermes CLI/setup/provider runtime guidance

Implication for Belltower:

- keep auth/config/provider routing centralized
- keep readiness, login, setup, and model selection coherent as one subsystem
- do not let provider quirks deform unrelated layers

### 2.4 Onboarding, setup, and readiness

One nuance that appears repeatedly in Hermes, Gemini, Qwen, and other
productized harnesses is that first-run setup and ongoing readiness are not
"just config."

They usually become a real operator subsystem covering:

- login
- provider selection
- model selection
- environment checks
- setup-time guardrails
- status and doctor surfaces

Implication for Belltower:

- `setup`, `login`, `model`, `doctor`, and `status` should continue to be
  treated as one coherent product surface
- provider/model readiness should remain truthful and probe-backed
- future editor or ACP-style clients should reuse the same readiness picture
  rather than inventing parallel setup paths

### 2.5 Tools, sandbox, policy, and approvals

The stronger harnesses do not blur tool execution, shell access, sandboxing,
and approval policy into one vague layer.

Patterns that recur:

- Codex splits tools, shell command execution, shell escalation, and sandboxing
- OpenCode exposes role and permission differences directly in product framing
- Gemini, Qwen, Goose, and others all surface policy/sandbox/runtime
  environment choices explicitly

Implication for Belltower:

- tools, approvals, sandbox policy, and execution policy deserve explicit
  subsystem ownership
- delegated work should not inherit unsafe approvals implicitly
- MCP, built-ins, shell, and future workflow tools should all traverse one
  inspectable policy layer

### 2.6 Storage, sessions, sync, and history

A durable pattern is explicit session and transcript ownership.
Even when the underlying storage differs, the best products make session state
visible as a real subsystem.

This is clearest in:

- pi session/tree/fork/compaction behavior
- Codex thread/turn/item lifecycle
- Grok storage modules for sessions, transcript views, and tool results
- OpenCode session and sync surfaces

Implication for Belltower:

- Belltower should keep treating the canonical session/event store as the
  center of gravity
- transcript, workflow, tool-call inspection, and branch/session lineage should
  remain queryable, not UI-local
- unlike much of the field, Belltower does not need to weaken this seam to
  match other harnesses
- the survey suggests Belltower is already unusually strong here, so the right
  move is to harden the canonical store path rather than conform downward

### 2.7 Workflow and delegation

The broadest repeated lesson in this survey is that workflow/delegation grows
into a subsystem of its own.

It appears as:

- built-in agents and remote-driving model in OpenCode
- scheduler, verify, headless, and delegation surfaces in Grok
- subagent and task isolation in oh-my-pi and Every Code
- batch, cron, gateway, ACP, and auxiliary execution surfaces in Hermes

Implication for Belltower:

- workflow and subagents should not be treated as "just more transcript"
- they need their own control plane, bounded state, isolation rules, and
  inspection surfaces
- worktree identity and parent/child session lineage should remain first-class
- some harnesses push delegation into extensions instead of the core; Belltower
  should choose that boundary intentionally rather than drifting into one by
  convenience

### 2.8 Extensibility taxonomy

A useful distinction across mature harnesses is that extensibility is not one
generic plugin bucket.

The better systems separate:

- tools
- MCP
- skills
- commands
- hooks
- prompts
- themes
- packages or plugins

Implication for Belltower:

- extensibility surfaces should be categorized by capability
- skills, MCP, memory providers, and future plugins should not all share one
  blurry boundary

## 3. Harness Family Lessons

### 3.1 Codex and Every Code

These are the strongest donors for hard boundaries and control-plane
discipline.

Important sources:

- [`/tmp/belltower-case-studies/harness/codex/README.md`](/tmp/belltower-case-studies/harness/codex/README.md)
- [`/tmp/belltower-case-studies/harness/codex/codex-rs/README.md`](/tmp/belltower-case-studies/harness/codex/codex-rs/README.md)
- [`/tmp/belltower-case-studies/harness/codex/codex-rs/Cargo.toml`](/tmp/belltower-case-studies/harness/codex/codex-rs/Cargo.toml)
- [`/tmp/belltower-case-studies/harness/codex/codex-rs/protocol/README.md`](/tmp/belltower-case-studies/harness/codex/codex-rs/protocol/README.md)
- [`/tmp/belltower-case-studies/harness/codex/codex-rs/app-server/README.md`](/tmp/belltower-case-studies/harness/codex/codex-rs/app-server/README.md)
- [`/tmp/belltower-case-studies/harness/code/README.md`](/tmp/belltower-case-studies/harness/code/README.md)

What to copy:

- explicit app-server and protocol boundary
- clear split between runtime logic, wire types, and UI shells
- auth/config/secrets isolation
- sandbox and execution policy as explicit product features
- support for both interactive and headless or programmatic execution

What to adapt:

- Codex app-server breadth is useful, but Belltower should keep its HTTP/SSE
  surface smaller and more event-centered
- Every Code's workflow richness is useful, but Belltower should route it
  through explicit session/workflow semantics rather than informal task state

What to avoid:

- letting protocol growth outrun clarity
- stuffing too many unrelated operational concerns into one giant control plane
  without equally strong docs and type boundaries

### 3.2 pi-mono and oh-my-pi

These are the strongest donors for session/tree semantics, queueing,
compaction, and extension taxonomy.

Important sources:

- [`/tmp/belltower-case-studies/harness/pi-mono/pi-mono-full/README.md`](/tmp/belltower-case-studies/harness/pi-mono/pi-mono-full/README.md)
- [`/tmp/belltower-case-studies/harness/pi-mono/pi-mono-full/packages/coding-agent/README.md`](/tmp/belltower-case-studies/harness/pi-mono/pi-mono-full/packages/coding-agent/README.md)
- [`/tmp/belltower-case-studies/harness/pi-mono/pi-mono-full/packages/coding-agent/docs/rpc.md`](/tmp/belltower-case-studies/harness/pi-mono/pi-mono-full/packages/coding-agent/docs/rpc.md)
- [`/tmp/belltower-case-studies/harness/pi-mono/pi-mono-full/packages/coding-agent/docs/extensions.md`](/tmp/belltower-case-studies/harness/pi-mono/pi-mono-full/packages/coding-agent/docs/extensions.md)
- [`/tmp/belltower-case-studies/harness/oh-my-pi/README.md`](/tmp/belltower-case-studies/harness/oh-my-pi/README.md)
- [`/tmp/belltower-case-studies/harness/oh-my-pi/AGENTS.md`](/tmp/belltower-case-studies/harness/oh-my-pi/AGENTS.md)

What to copy:

- sessions, tree navigation, forking, and compaction as real product features
- queueing and interruptibility as explicit operator behavior
- package-level separation between provider API, agent core, coding agent, TUI,
  and web UI
- extension categories richer than a single plugin concept

What to adapt:

- pi's behavior and invariants are worth borrowing more than its storage
  substrate
- oh-my-pi's config discovery and task/subagent ideas are useful, but should
  remain inspectable and bounded in Belltower

What to avoid:

- copying JSONL or file-tree storage directly
- allowing cross-tool config discovery to become magical or hard to reason
  about
- drifting into subagents by accretion instead of designing a workflow
  subsystem deliberately

### 3.3 OpenCode

OpenCode is a strong donor for client/server framing, control-plane thinking,
and worktree-aware remote driving.

Important sources:

- [`/tmp/belltower-case-studies/harness/opencode/README.md`](/tmp/belltower-case-studies/harness/opencode/README.md)
- [`/tmp/belltower-case-studies/harness/opencode/package.json`](/tmp/belltower-case-studies/harness/opencode/package.json)
- [`/tmp/belltower-case-studies/harness/opencode/packages/opencode/specs/v2.md`](/tmp/belltower-case-studies/harness/opencode/packages/opencode/specs/v2.md)
- [`/tmp/belltower-case-studies/harness/opencode/packages/opencode/src/acp/README.md`](/tmp/belltower-case-studies/harness/opencode/packages/opencode/src/acp/README.md)
- [`/tmp/belltower-case-studies/harness/opencode/packages/opencode/src/sync/README.md`](/tmp/belltower-case-studies/harness/opencode/packages/opencode/src/sync/README.md)

What to copy:

- explicit statement that the TUI is just one client
- control-plane framing for remote driving, sync, and workspace/worktree
  operations
- read-only versus full-access agent roles as visible product behavior
- multiple shells over a shared core

What to adapt:

- OpenCode's breadth suggests a strong future direction for Belltower, but
  Belltower should land core harness correctness before multiplying surfaces

What to avoid:

- surface sprawl without crisp ownership
- allowing sync/share/desktop/web features to outrun the canonical session
  model

### 3.4 Goose

Goose is a strong donor for Rust workspace decomposition and product-shell
separation.

Important sources:

- [`/tmp/belltower-case-studies/harness/goose/README.md`](/tmp/belltower-case-studies/harness/goose/README.md)
- [`/tmp/belltower-case-studies/harness/goose/Cargo.toml`](/tmp/belltower-case-studies/harness/goose/Cargo.toml)

What to copy:

- clear workspace-level package decomposition
- separate CLI, server, SDK, MCP, and UI surfaces
- making packaging/distribution a first-class architecture concern

What to adapt:

- Goose's custom distro story is a reminder that packaging and extension
  presets matter; for Belltower this should inform setup/profiles later, not
  distract from the canonical runtime now

What to avoid:

- treating a multi-surface workspace as sufficient evidence of good ownership
  without checking the actual seams

### 3.5 Gemini CLI, Qwen Code, and Grok CLI

This group is useful as evidence that modern harnesses frequently converge on a
shared-core monorepo plus workflow and service layers.

Important sources:

- [`/tmp/belltower-case-studies/harness/gemini-cli/README.md`](/tmp/belltower-case-studies/harness/gemini-cli/README.md)
- [`/tmp/belltower-case-studies/harness/gemini-cli/package.json`](/tmp/belltower-case-studies/harness/gemini-cli/package.json)
- [`/tmp/belltower-case-studies/harness/gemini-cli/packages/core/package.json`](/tmp/belltower-case-studies/harness/gemini-cli/packages/core/package.json)
- [`/tmp/belltower-case-studies/harness/qwen-code/package.json`](/tmp/belltower-case-studies/harness/qwen-code/package.json)
- [`/tmp/belltower-case-studies/harness/qwen-code/packages/core/package.json`](/tmp/belltower-case-studies/harness/qwen-code/packages/core/package.json)
- [`/tmp/belltower-case-studies/harness/grok-cli/README.md`](/tmp/belltower-case-studies/harness/grok-cli/README.md)

What to copy:

- shared core package with thin shells
- explicit services for auth, provider routing, sessions, and worktrees
- workflow isolation for verify, scheduler, headless, and remote-control flows

What to adapt:

- the strong provider/config surfaces are useful examples, but Belltower should
  resist pushing provider-specific complexity into unrelated crates

What to avoid:

- letting auth/provider permutations dictate the runtime architecture
- adding workflow modes without equally strong inspection and bounded-state
  rules

### 3.6 Hermes Agent and Claude Code

These are most useful as large-product references for breadth, operator
ergonomics, and subsystem enumeration.

Important sources:

- [`/tmp/belltower-case-studies/harness/hermes-agent/website/docs/developer-guide/architecture.md`](/tmp/belltower-case-studies/harness/hermes-agent/website/docs/developer-guide/architecture.md)
- [`/tmp/belltower-case-studies/harness/hermes-agent/website/docs/developer-guide/gateway-internals.md`](/tmp/belltower-case-studies/harness/hermes-agent/website/docs/developer-guide/gateway-internals.md)
- [`/tmp/belltower-case-studies/harness/hermes-agent/website/docs/developer-guide/memory-provider-plugin.md`](/tmp/belltower-case-studies/harness/hermes-agent/website/docs/developer-guide/memory-provider-plugin.md)
- [`/tmp/belltower-case-studies/harness/hermes-agent/website/docs/reference/tools-reference.md`](/tmp/belltower-case-studies/harness/hermes-agent/website/docs/reference/tools-reference.md)
- [`/tmp/belltower-case-studies/harness/hermes-agent/website/docs/reference/toolsets-reference.md`](/tmp/belltower-case-studies/harness/hermes-agent/website/docs/reference/toolsets-reference.md)
- [`/tmp/belltower-case-studies/harness/hermes-agent/website/docs/reference/skills-catalog.md`](/tmp/belltower-case-studies/harness/hermes-agent/website/docs/reference/skills-catalog.md)
- [`/tmp/belltower-case-studies/harness/claude-code/src`](/tmp/belltower-case-studies/harness/claude-code/src)

What to copy:

- Hermes's subsystem-level docs
- gateway, cron, ACP/editor integration, and memory provider seams
- product clarity around setup, model/provider resolution, tools, and skills

What to adapt:

- both repos suggest large, capable product surfaces; Belltower should copy the
  subsystem concept and operator story without inheriting monolithic core
  ownership

What to avoid:

- assuming folder separation alone is enough
- shipping a huge memory/plugin ecosystem before the canonical runtime and event
  model are stable

### 3.7 Aider

Aider is a strong workflow and product reference, but a weaker architecture
donor for Belltower's intended direction.

Important sources:

- [`/tmp/belltower-case-studies/harness/aider/README.md`](/tmp/belltower-case-studies/harness/aider/README.md)
- [`/tmp/belltower-case-studies/harness/aider/pyproject.toml`](/tmp/belltower-case-studies/harness/aider/pyproject.toml)

What to copy:

- direct coding workflow polish
- git-aware interaction and high-value product ergonomics

What to avoid:

- using it as the main architecture donor for Belltower, because it does not
  reinforce the server/client and canonical-session shape Belltower is aiming
  for

## 4. Copy / Adapt / Reject Matrix

| Area | Copy | Adapt | Reject |
| --- | --- | --- | --- |
| Protocol and server/client boundary | Codex app-server and protocol discipline | Keep Belltower's wire surface smaller and more event-first | Embedded or UI-bypassing local execution paths |
| Session, tree, queue, compaction | pi's product model | Keep Belltower's SQLite event store instead of JSONL | Treating branching/history as UI-only behavior |
| Workflow and delegation | OpenCode, Grok, oh-my-pi, Every Code | Route through explicit parent/child session lineage and worktree metadata | Smuggling subagents into transcript logic or TUI-local state |
| Auth/config/provider routing | Codex and Gemini/Qwen isolation | Keep Belltower provider surface narrower until readiness matures | Provider logic smeared across runtime and UI |
| Tools, sandbox, approvals | Codex separation of tools, shell, escalation, sandbox | Keep one Belltower policy layer across built-ins, MCP, shell, and future workflow tools | Ad hoc approval inheritance or hidden unsafe paths |
| Extensibility | pi and Hermes taxonomy of skills, tools, prompts, hooks, memory providers | Keep capability categories explicit in Belltower | One undifferentiated plugin bucket |
| Operator surfaces | Goose/OpenCode/Codex multi-shell model | Expand only on top of `bt-protocol` and `bt-client` | Surface proliferation that outruns core runtime correctness |

## 5. Main Conclusions

The clearest repeated lesson is that the stronger harnesses converge on the
same high-level architectural shape:

1. a reusable runtime or agent core
2. an explicit transport or control plane
3. coherent onboarding/setup/readiness surfaces
4. separate operator surfaces
5. a real storage/session subsystem
6. explicit policy, sandbox, and approval layers
7. workflow/delegation treated as a subsystem of its own
8. categorized extensibility instead of one generic plugin story

Belltower is already pointed in the right direction on several of these:

- server-first transport
- canonical session store
- event-first telemetry
- crate-level subsystem separation

The survey also suggests one place where Belltower should lead rather than
borrow:

- the canonical session/event-store seam is more explicit in Belltower's
  architecture than in most of the surveyed harnesses

The harness survey mainly reinforces two things:

1. these seams should get stricter, not looser
2. workflow/delegation, onboarding/readiness, and operator-surface usability
   are where architectural sloppiness multiplies fastest

## 6. Belltower Audit Checklist

Use this list while auditing or extending Belltower:

- Is the behavior owned by the right subsystem, or is it smeared across
  runtime, server, and TUI?
- Does the feature ride the canonical protocol boundary, or does it bypass it?
- Is session, branch, workflow, or subagent state durable and inspectable?
- Are tools, approvals, sandboxing, and execution policy routed through one
  coherent layer?
- Is provider/auth/config complexity isolated, or is it leaking through the
  product?
- Is the extension surface categorized by capability?
- Would a future web, editor, or automation client be able to use the same
  control plane?
- If this is workflow or delegation, is it its own subsystem with bounded
  state, lineage, and isolation?
- If this is usability work, is it improving a real subsystem seam or just
  patching around missing architecture?

## 7. Recommended Follow-On

The next useful Belltower-specific follow-on documents are:

1. a sharpened subsystem target map for workflow/delegation, tying together:
   - [`../architecture/subagents-and-session-graphs.md`](../architecture/subagents-and-session-graphs.md)
   - [`../architecture/participants-and-collaboration.md`](../architecture/participants-and-collaboration.md)
   - [`../subsystems/session-runtime-and-agent.md`](../subsystems/session-runtime-and-agent.md)
2. a usability audit organized by subsystem:
   - launcher and onboarding
   - protocol and inspection
   - tools and approvals
   - session, branch, and workflow navigation
   - TUI interaction
3. a Belltower extension taxonomy document that separates:
   - built-in tools
   - MCP tools
   - skills
   - memory providers
   - future plugins
