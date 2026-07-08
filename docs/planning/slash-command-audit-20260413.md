# Slash Command Audit — 2026-04-13

This note re-audits the live `bt-tui` slash-command surface after the TUI
cutover and the defaults/settings work.

It is not a deletion plan by itself. The goal is to define which commands
should stay prominent in the initial operator surface, which commands should be
demoted but kept implemented, and which overlaps should be folded later.

## Scope

Primary implementation references:

- [`../../crates/bt-tui/src/commands.rs`](../../crates/bt-tui/src/commands.rs)
- [`../../crates/bt-tui/src/command_actions.rs`](../../crates/bt-tui/src/command_actions.rs)
- [`../../crates/bt-tui/src/command_render.rs`](../../crates/bt-tui/src/command_render.rs)

Relevant product references:

- [`../subsystems/launcher-and-tui.md`](../subsystems/launcher-and-tui.md)
- [`../subsystems/tui-architecture.md`](../subsystems/tui-architecture.md)
- [`./slash-command-audit-20260405.md`](./slash-command-audit-20260405.md)

## Current Live Surface

The current registry exposes 31 canonical commands:

- `/help`
- `/status`
- `/models`
- `/doctor`
- `/mcp [reload]`
- `/session`
- `/inspect [session|tool <call-id>]`
- `/history [limit]`
- `/execution [limit]`
- `/raw [diff] [turn-id] [call <n>] [limit]`
- `/usage [limit]`
- `/queue [clear]`
- `/branches`
- `/tree`
- `/lineage`
- `/workflow`
- `/export [bundle|jsonl|html|sharegpt|otlp]`
- `/refresh`
- `/new`
- `/compact`
- `/resume [session-id]`
- `/spawn <objective>`
- `/branch`
- `/cancel`
- `/detach`
- `/steer <message>`
- `/approve [once|session|always]`
- `/deny [once|session|always]`
- `/connection [id]`
- `/model [model-id]`
- `/defaults [connection <id>|model <model-id>|use <connection> [model]]`
- `/mode [standard|extended]`
- `/use <connection> [model]`

Aliases remain:

- `/r` -> `/refresh`
- `/delegate` -> `/spawn`
- `/fork` -> `/branch`
- `/stop` -> `/cancel`
- `/redirect` -> `/steer`
- `/yes` -> `/approve`
- `/no` -> `/deny`
- `/conn` -> `/connection`
- `/mod` -> `/model`
- `/default` -> `/defaults`

## Findings

### 1. The top-level surface is too wide for the default menu

The registry is structurally sound, but 31 canonical commands is too many for
the default slash menu and default `/help` output.

The current menu is trying to expose:

- primary session lifecycle
- provider/readiness configuration
- runtime control-plane actions
- graph inspection
- low-level execution inspection
- export/debug surfaces

That is too much top-level vocabulary for a first-pass operator surface.

### 2. The current surface mixes core operator flows with advanced diagnostics

These commands map directly to frequent operator needs and deserve prominent
placement:

- create or reopen work
- select provider/model defaults
- inspect the current session and queued state
- resolve approvals
- cancel or steer active work

These commands are valid, but they are advanced inspection or debugging tools:

- `/raw`
- `/execution`
- `/usage`
- `/export`
- `/workflow`
- `/lineage`
- `/compact`

They should remain implemented, but they should not dominate first-run
discoverability.

### 3. There is real overlap in the provider/configuration family

Current family:

- `/status`
- `/models`
- `/doctor`
- `/connection`
- `/model`
- `/defaults`
- `/mode`
- `/use`

This is too many top-level entries for what is conceptually one subsystem.

The coherent operator story is:

- `/use` is the main session-level choice
- `/status` is the readiness/truth view
- `/defaults` is the future-session default view

The direct setters `/connection`, `/model`, and `/mode` are power-user controls.
They can stay implemented, but they should be demoted from the default menu.

### 4. Inspection is fragmented across too many siblings

Current inspection family:

- `/session`
- `/inspect`
- `/history`
- `/execution`
- `/raw`
- `/usage`
- `/queue`
- `/export`

This is functionally rich but too flat.

The strongest top-level commands here are:

- `/session`
- `/inspect`
- `/queue`

The others should be treated as advanced inspection/debug commands until they
are folded into a clearer namespace.

### 5. Graph/session-topology commands are over-exposed

Current graph family:

- `/branches`
- `/tree`
- `/lineage`
- `/workflow`
- `/branch`
- `/spawn`

This is more graph vocabulary than the initial TUI needs to expose by default.

The core operator action here is `/branch`.
The rest are useful inspection or delegation tools, but they should be demoted
until the session-graph/subagent experience is more mature.

### 6. The previous audit is already stale

The older audit note from `2026-04-05` predates `/defaults`.
The live implementation now includes:

- `/defaults`
- `/default` alias
- `/defaults` completion and command-menu support
- `/defaults` in the active-turn allowlist

That means the command surface is changing fast enough that cleanup should rely
on the live registry, not prior inventory notes.

### 7. Command metadata is still split across multiple decision points

The registry is centralized for naming and descriptions, which is good.
But visibility and behavior policy still live in separate logic:

- `command_allowed_during_request(...)`
- command-specific completion branches
- command-menu branching logic
- help output rendering

For cleanup, the next step should not be manual pruning in several places.
The registry should become the source of truth for:

- family
- visibility tier
- whether the command is advanced
- whether the command is allowed during an active request

## Recommended Starting Surface

These commands should remain prominent in the default slash menu and default
`/help` output.

### Tier 1: keep prominent

- `/help`
- `/use`
- `/status`
- `/defaults`
- `/session`
- `/inspect`
- `/history`
- `/usage`
- `/new`
- `/branch`
- `/cancel`
- `/mcp`
- `/models`
- `/connection`
- `/compact`

This set keeps the top-level surface focused on:

- discoverability
- provider/model readiness
- session truth
- compact observability that still helps normal operation
- lifecycle
- in-flight control

## Recommended Demotion Set

These commands should remain implemented and callable by exact name, but should
be hidden from the default slash menu and default `/help` surface.

### Tier 2: advanced, hidden by default

- `/doctor`
- `/model`
- `/mode`
- `/execution`
- `/raw`
- `/export`
- `/branches`
- `/tree`
- `/lineage`
- `/workflow`
- `/refresh`
- `/resume`
- `/spawn`
- `/steer`
- `/detach`

### Contextual-only controls

These are still first-class operator actions, but they should not be top-level
slash commands:

- approval resolve/deny
- queued follow-up state

Current examples:

- `/approve` and `/deny` should resolve through the approval chooser instead
  of through the slash surface
- `/queue` should be represented in the normal control-plane flow instead of as
  a top-level slash command

These commands are still useful.
They just do not all deserve equal prominence in the first operator surface.

## Proposed Fold Targets

These are not immediate command deletions.
They are the direction the command surface should move toward.

### Configuration family

- keep `/use` as the primary session-level configuration command
- keep `/defaults` as the future-session configuration command
- keep `/status` as the primary readiness report
- demote `/connection`, `/model`, and `/mode`

### Inspection family

Eventually fold these under `/inspect` or another explicit advanced namespace:

- `/history`
- `/execution`
- `/raw`
- `/usage`
- `/export`

### Graph family

Eventually fold these under an explicit graph/session-topology namespace:

- `/branches`
- `/tree`
- `/lineage`
- `/workflow`
- `/spawn`

### Maintenance/debug family

- `/refresh`
- `/compact`

These are real tools, but they read as advanced maintenance rather than normal
session operation.

## Recommended Cleanup Order

### Phase 1: visibility cleanup only

- add visibility metadata to `CommandDef`
- keep all handlers intact
- keep all wire/client behavior intact
- hide Tier 2 commands from the default slash menu
- change default `/help` to show Tier 1 only
- add an explicit advanced help surface such as `/help all`

### Phase 2: family cleanup

- move readiness/configuration commands toward `/use`, `/status`, and `/defaults`
- demote direct setter commands further
- decide whether `/inspect` should become the primary advanced inspection root

### Phase 3: namespace consolidation

- fold graph-oriented commands into one coherent family
- fold low-level inspection/debug commands into one coherent family
- only remove top-level spellings after the replacement surface is stable

## Working Recommendation

Do not delete advanced commands yet.

The correct near-term cleanup is:

1. keep the current implementation surface intact
2. reduce what the default menu and default `/help` present
3. move the operator mental model toward a smaller Tier 1 set
4. consolidate deeper inspection and graph commands later, once the replacement
   surface is clear

That keeps Belltower honest as an actively developing system while still making
the TUI easier to learn and easier to operate.
