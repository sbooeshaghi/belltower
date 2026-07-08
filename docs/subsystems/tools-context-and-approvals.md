# Tools, Context, and Approvals

This document covers:

- `bt-tools`
- `bt-context`
- the approval parts of `bt-runtime`

The most useful `pi` patterns here are tool guardrails, file mutation serialization, and compaction invariants.

Read this together with:

- [`../architecture/tool-system-and-normalization.md`](../architecture/tool-system-and-normalization.md)
- [`../architecture/tool-policy.md`](../architecture/tool-policy.md)
- [`../architecture/claude-code-lessons-and-adoption.md`](../architecture/claude-code-lessons-and-adoption.md)

## Purpose

This layer determines whether Belltower behaves like a trustworthy harness or a fragile shell wrapper.

It is responsible for:

- built-in tool behavior
- guardrails and safety
- approval policies and persistence
- context window management
- compaction and summarization
- instruction and skill loading

## Built-In Tools

The stable harness should provide at least:

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

The current `search` implementation now uses the native ripgrep crate
stack for repository-scale content and path search rather than shelling
out to `rg`. External web research is split into `web_search` (ranked
source discovery) and `web_fetch` (specific public URL retrieval with
status, truncation, and content-digest metadata).

The current initial-release mode inventories are intentionally explicit:

- `standard`: `read`, `write`, `edit`, `list`, `search`, `shell`
- `extended`: `standard` plus `web_search`, `web_fetch`, `plan`, `ask`, `inspect`

There is no separate autonomous session mode in the current public surface.
If Belltower later adds one, it should only appear once it has distinct runtime semantics and a wider tool inventory.

Every built-in tool must integrate with:

- session events
- approval policy
- runtime control
- exportability

The current implementation now also expects every canonical tool to carry typed
metadata describing:

- risk
- read-only status
- concurrency safety
- interrupt behavior
- deferred-discovery status
- display grouping and catalogue tags

That metadata belongs in `bt-core` because it is runtime policy data, not just a
UI hint.

The wider policy model for visibility, approval, boundedness, concurrency, and
interrupt behavior is defined in:

- [`../architecture/tool-policy.md`](../architecture/tool-policy.md)

Inspection tools now follow the same tool contract rather than a second introspection path.

That means:

- canonical inspection/query services live in runtime/server
- agent-facing self-inspection is exposed through normal read-only tools
- inspection tools use the same tool-call and tool-result event flow as any other tool
- the broad model-facing inspection built-in is a single `inspect` tool with typed queries instead of a large family of separate `inspect_*` names
- `session_search` is the narrow exact-text session-history search tool; it
  returns canonical event/turn/tool references and must not grow into hidden
  memory or semantic retrieval

This keeps human and agent inspection aligned while preserving one auditable tool system.

The other important boundary is:

- built-in canonical names should be short and stable
- operational commands can still exist through `shell` and `bt ...`
- implicit session context should still be injected when it is cheaper and more truthful than a tool call

### Web Retrieval Providers

`web_search` and `web_fetch` are provider-backed tools, but providers
must sit behind the canonical tool schema.

Current foundation backends:

- `web_search`: Exa, selected through `[web].search_backend`, the
  `BELLTOWER_WEB_SEARCH_BACKEND` override, or an explicit tool backend
  argument
- `web_fetch`: direct public HTTP/HTTPS fetch with explicit
  localhost/private-address blocking, redirect validation, and byte-bounded
  response reads before truncation

Operator setup follows the same configuration/auth-store pattern as
model providers:

```bash
belltower web status
belltower web configure exa
belltower web login exa --api-key-env EXA_API_KEY
```

`belltower web login exa` stores credentials under the synthetic
provider id `web:exa`. If no stored credential is present, Exa also
checks the configured environment sources `EXA_API_KEY` and
`BELLTOWER_EXA_API_KEY`. This keeps backend choice and credential
resolution explicit without changing the model-facing `web_search`
schema.

Future providers such as Brave, Tavily, SearXNG, Firecrawl, DDGS, or
browser-backed retrieval can be added one backend at a time. They must
return the same canonical result shapes and remain visible through the
normal tool-call, approval, inspection, and export path.

`web_fetch` boundedness is part of the tool contract, not just display polish:
the direct backend stops reading once the configured byte cap is reached, then
returns a stable content digest and an explicit `truncated` flag. Future fetch
backends must preserve that bounded retrieval behavior so web evidence remains
safe to inspect and export.

## Guardrails

This is one of the best places to copy mature behavior from `pi`.

Required guardrails include:

- project-root enforcement
- `.gitignore` awareness where appropriate
- binary detection
- output truncation
- per-file mutation serialization
- timeout and cancellation
- process-tree cleanup for shell commands

Important rule:

- guardrails are not optional UX polish; they are part of the harness contract

## Approval Model

Approvals must be first-class and persisted.

The runtime must support:

- policy auto-approval
- explicit human approve
- explicit human deny
- pending state with turn suspension
- replay of approval decisions
- explicit human-input pauses through `ask`

The current approval path now uses approval requirement plus tool metadata
instead of only hardcoded tool-name checks.
That is the right direction.

Approval requests now persist a durable request snapshot, not just the fact that
something is pending. The snapshot records the request fingerprint, an argument
hash, a redacted argument preview, the approval requirement, tool metadata,
initiator, surface, and optional registry/policy identifiers as they existed at
request time. Approval resolution is a separate durable record tied back to the
request fingerprint. This distinction matters: an old approval must remain
explainable and resumable even if the live tool registry, MCP inventory, or
policy wording changes later.

The executable tool arguments still come from the canonical `tool.call.requested`
event. Redacted approval previews are audit/UI evidence only and must not become
the source used to execute a resumed tool.

`ask` is now a real native tool rather than a UI-only shortcut:

- the agent emits a canonical `ask` tool call
- the runtime persists the paused turn in `waiting_on_input`
- the TUI answers through the normal server/runtime path
- the resumed turn continues with a normal tool result message for that `ask` call

This keeps human-input pauses aligned with approvals while keeping the two pause reasons distinct.

The TUI must make the pending state visible and resolvable without guesswork.

## Context and Compaction

Belltower must support long-running sessions.

That means:

- token budgeting
- reserve-window logic
- summarization
- file-read/file-modified tracking
- branch-aware summaries
- compaction correctness

Important invariants copied from `pi` in spirit:

- tool calls and tool results must not be separated
- file tracking must survive compaction
- summaries must preserve actionable work state
- the compacted request must actually fit the target budget

## Instructions and Skills

v1 should stay minimal:

- managed built-in harness prompt assets
- generated runtime context
- generated tool-guidance sections
- global markdown skills
- project markdown skills
- deterministic composition
- reconstructable active instruction set

The built-in harness prompt also carries a validation discipline: for
file-producing, data-processing, and exact-output tasks, the agent should inspect
local instructions, tests, schemas, fixtures, or expected-output files when they
exist. When runtime context lists validation surfaces, the agent should inspect
the relevant surfaces before using `write` or `edit` for exact-output artifacts,
then run the smallest relevant validation command before finalizing. This is not
benchmark-specific behavior; it is part of Belltower's reproducibility contract
for scientific and engineering work.

For tabular or structured scientific data, the prompt also tells the agent to
align records by explicit identifiers such as dates, sample IDs, filenames, or
primary keys rather than assuming row order means correspondence. That rule is
part of the same reproducibility contract: it keeps data-processing behavior
general without hardcoding benchmark-specific answers.

The generated runtime context may also list detected validation surfaces such as
`README.md`, `tests/`, `schemas/`, `fixtures/`, or expected/golden-output files.
This is intentionally a hint inside the canonical prompt, not hidden execution:
the agent still uses normal tools to inspect those files and the rendered prompt
is captured in `turn.instructions.recorded`.

Validation-surface detection is deliberately bounded and path-only. `bt-context`
walks a small deterministic subset of the project tree, skips generated or
hidden directories such as `.git`, `.belltower`, `target`, and `node_modules`,
and records only relative paths. It does not read file contents or replace the
normal instruction resolver.

When `write` or `edit` is requested before the relevant listed validation
surface has been inspected, the server-side tool adapter returns a recoverable
tool error instead of mutating the workspace. Check surfaces such as tests,
specs, schemas, fixtures, expected outputs, and golden outputs are stricter than
instruction surfaces such as `README.md`: if a concrete check surface exists,
reading only the instruction surface is not enough to unlock mutation. The
preflight also tracks read results inside the active turn, because a model may
read validation files and write an artifact in the same provider/tool loop
before those intermediate tool messages are durably replayable as branch
history. That preflight is intentionally outside `bt-tools`: it depends on
session and turn history, while the built-in file tools remain mechanical
filesystem operations.

Each turn now records canonical instruction provenance in
`turn.instructions.recorded`. That event stores the resolved core prompt,
provider overlay, ordered markdown instructions, and exact rendered system
prompt used for the request. Reconstruction reads the session event log rather
than rereading mutable prompt or skills files from disk.

`cwd` in the prompt means the session working directory, which in v1 is `session.project_root` because shell execution is rooted there.

This is intentionally smaller than a general plugin system.

## Human Checkpoints

1. ask the agent to read a file
2. ask it to run a shell command requiring approval
3. approve once and deny once
4. run a long enough session to force compaction
5. verify the agent still retains actionable context afterward

## Current Priorities

- expand runtime use of tool metadata beyond approval, pause semantics, and deferred discovery
- improve approval rule richness and inspection
- continue hardening `plan`, `ask`, and `inspect` operator flows
- keep MCP external, unaliased, and discoverable through `catalogue`
- continue hardening tool guardrails and tool-result structure
