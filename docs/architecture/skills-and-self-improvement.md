# Skills And Self-Improvement

This document defines how Belltower should implement skills, and how that skill system should eventually connect to reviewable self-improvement.

Read it with:

- [`./overview.md`](./overview.md)
- [`./tool-system-and-normalization.md`](./tool-system-and-normalization.md)
- [`./datasets-and-training-artifacts.md`](./datasets-and-training-artifacts.md)
- [`../subsystems/tools-context-and-approvals.md`](../subsystems/tools-context-and-approvals.md)

It exists because Belltower already has:

- a canonical session history
- managed harness prompt assets
- global and project markdown instruction overlays
- an experimental `bt-improve` crate

What it does not yet have is the middle layer between one-off session behavior and first-class native tools:

- reusable procedural memory
- reviewable skill authoring
- structured improvement workflows that learn from prior sessions without silently mutating the harness

## Current State

Today Belltower has only a minimal markdown-based instruction system:

- managed built-in prompt assets from `data/prompts/`
- global markdown instructions under the global skills directory
- project markdown instructions under `.belltower/skills`
- deterministic composition into the system prompt

This is useful, but it is still flatter than a real skill system.

Right now there is no first-class distinction between:

- a general project instruction
- a reusable workflow skill
- a candidate improvement derived from prior session history

This document defines that missing structure.

## Terminology

### Tool

A tool is a model-callable capability with:

- a canonical name
- a structured schema
- approval and risk metadata
- an executable implementation

Examples:

- `read`
- `search`
- `plan`
- `inspect`

### Skill

A skill is reusable procedural guidance for a recurring class of tasks.

A skill is not a privileged runtime capability.

A skill may contain:

- instructions
- references
- templates
- scripts
- examples

Skills are procedural memory, not execution privileges.

### Prompt Asset

A prompt asset is harness-owned static guidance.

Examples:

- `data/prompts/00-core.md`
- `data/prompts/provider/openai-compatible.md`

Prompt assets define the harness contract, not user or project workflows.

### Memory

Memory is factual or preference-oriented recall.

Examples:

- user preferences
- project context
- prior decisions

Memory is not the same thing as a skill.

### Improvement

Improvement is a review workflow that examines prior sessions and produces one of:

- a skill draft
- a skill patch
- a prompt-asset patch
- a native-tool proposal

Improvement should be reviewable and auditable.

It should not begin as unrestricted self-modification.

## Why Skills Matter

Skills are useful because they let Belltower preserve successful workflows without forcing every repeated pattern into:

- a bigger system prompt
- a native Rust tool
- a one-off session note

That gives Belltower a clean middle layer:

- prompt assets define the harness
- tools define executable capabilities
- skills define reusable procedures
- memory preserves facts and preferences

This is the right shape for a harness that wants to become better over time without becoming opaque or ad hoc.

## Design Principles

### 1. Skills Are Procedural Memory

Skills should encode:

- when a workflow applies
- how to execute it
- what pitfalls to avoid
- how to verify success

They should not encode:

- secrets
- unstable session-local state
- hidden approvals
- arbitrary imperative behavior outside the normal tool/runtime model

### 2. Skills Stay Reviewable

In Belltower, the first acceptable self-improvement loop is:

- inspect prior work
- draft an artifact
- review it
- apply it intentionally

Not:

- agent silently edits its own operating procedures without review

### 3. Skills Are Progressive-Disclosure Context

The system prompt should not load every skill.

Instead, skills should be:

- listed cheaply
- opened on demand
- optionally attached to a turn or workflow

This preserves prompt budget and keeps cache behavior sane.

### 4. Skills And Tools Must Stay Distinct

If a recurring pattern is best expressed as:

- a reusable sequence of judgment and guidance

then it is probably a skill.

If it is best expressed as:

- a stable executable operation with a schema, approval boundary, and structured output

then it is probably a tool.

### 5. Improvement Should Prefer Skills First

When reviewing a session, the default improvement target should be:

- a skill draft or skill patch

not:

- a new native tool

Tool creation has a much higher bar because it implies:

- schema design
- approval policy
- runtime registration
- tests
- docs

## Skill Package Model

The skill package model should stay close to the current markdown discipline and expand it slightly.

Recommended structure:

```text
skills/
  some-skill/
    SKILL.md
    references/
    templates/
    scripts/
    assets/
```

`SKILL.md` is required.

Supporting directories are optional.

### `SKILL.md`

`SKILL.md` should contain:

- compact metadata frontmatter
- a short description
- trigger conditions
- numbered procedure
- pitfalls
- verification guidance

Suggested frontmatter shape:

```yaml
---
name: repo-summary
description: Summarize an unfamiliar repository quickly and truthfully.
version: 1
tags: [repo, onboarding, summary]
applies_when:
  - user asks for a repo or folder summary
  - the agent needs a repeatable first-pass survey workflow
---
```

The markdown body should stay readable by both humans and models.

## Phase 1: Read-Only Skills

The first real skill system for Belltower should be read-only.

That means:

- keep global skills under the global config root
- keep project skills under `.belltower/skills`
- preserve deterministic discovery order
- introduce first-class list and view surfaces

Recommended first capabilities:

- `skills_list`
  - list available skills with name and short description
- `skill_view`
  - load `SKILL.md`
  - optionally load a referenced file within the skill package

These should be model-callable, but read-only.

This phase gives Belltower:

- a real procedural-memory layer
- progressive disclosure
- a clear distinction between general instructions and reusable workflows

### Prompt Behavior In Phase 1

The normal harness prompt should continue to include:

- built-in prompt assets
- runtime context
- tool guidance
- instruction overlays
- active plan

Skills should not all be loaded automatically into every turn.

Instead:

- the operator may attach a skill
- a slash command may invoke a skill
- the model may call `skills_list` or `skill_view` when it needs procedural help

## Phase 2: Managed Skill Authoring

After read-only skills are solid, Belltower should add a managed write path.

This should start as:

- operator-driven
- proposal-based
- reviewable

Not as unrestricted autonomous editing.

Recommended write capabilities:

- create a new skill draft
- patch an existing skill draft
- update supporting files inside a skill package

These should first exist behind operator workflows or `bt-improve`, not as a broadly available always-on model tool.

The key rule is:

- Belltower should store improvement proposals before it applies them

That makes skill evolution inspectable and reversible.

## Phase 3: `/improve` As A Review Workflow

`/improve` should be the harness-level improvement workflow, not a magic command that edits the repo directly.

Its job should be:

1. inspect a session, branch, or selected range of turns
2. classify what kind of improvement opportunity exists
3. draft the smallest useful artifact
4. present it for review

### Inputs To `/improve`

`/improve` should review:

- turn history
- execution traces
- canonical errors
- tool usage patterns
- repeated operator commands
- optional raw provider evidence when relevant

This is why canonical events and `inspect` matter.

### Output Classes

`/improve` should classify opportunities into:

- skill candidate
- skill patch
- prompt-asset gap
- native-tool proposal
- docs gap
- no action

The default should be:

- draft a skill
- or patch a skill

Only when the gap is clearly structural should it draft a tool proposal.

### Why This Belongs In `bt-improve`

`bt-improve` is the right future home for this because it is:

- experimental
- outside the first compatibility promise
- naturally separate from the interactive runtime

The stable harness should produce the evidence.

`bt-improve` should consume that evidence and generate reviewable proposals.

## Phase 4: Self-Improving Skills

Only after the previous phases are working should Belltower allow more autonomous skill evolution.

Even then, the right model is constrained self-improvement, not unrestricted self-editing.

Recommended rules:

- skills may be patched automatically only in isolated review flows
- the resulting patch must be recorded as a proposal event or artifact
- the operator must be able to inspect, accept, reject, or revert it

Self-improving skills should be triggered by things like:

- repeated successful use of the same ad hoc workflow
- recurring error patterns while using a skill
- explicit user correction of a previously used skill
- post-session improvement runs over a completed session

This is the point where hook-style lifecycle triggers may become useful:

- `on_session_end`
- `on_error`
- `post_tool_call`

But the hooks should only initiate the review workflow.
They should not silently mutate live skills by themselves.

## Hermes Lessons

Hermes is useful here, but Belltower should borrow the right ideas rather than copying the whole system.

The important Hermes patterns are:

- skills are first-class procedural memory
- the agent can list and view them progressively
- there is a skill-management tool
- Hermes runs synthetic review turns that decide what is worth saving to memory or as skills before context is lost

That is a strong precedent for Belltower's future `/improve` flow.

What Belltower should not copy directly:

- unconstrained self-editing as the default
- a large implicit learning loop that is hard to audit
- mixing plugin, tool, memory, and skill semantics together

## Crate Ownership

The clean ownership model is:

### `bt-context`

Owns:

- skill-aware prompt composition
- attached-skill rendering into prompt context when a skill is explicitly invoked

### `bt-runtime`

Owns:

- skill discovery and deterministic resolution
- global/project skill loading
- later, narrow backends for read-only skill tools

### `bt-tools`

Owns:

- canonical read-only skill tools once they exist
- for example:
  - `skills_list`
  - `skill_view`

If write-capable skill tools are added later, they should still be canonical tool definitions here, backed by narrow runtime traits rather than direct server logic.

### `bt-session`

Owns:

- durable events and projections for:
  - skill attachment
  - skill proposal
  - skill application
  - skill rejection
  - later improvement runs

### `bt-server`

Owns:

- protocol routes
- slash-command and operator workflow plumbing
- review/apply surfaces for improvement proposals

### `bt-improve`

Owns:

- improvement analysis
- proposal generation
- later isolated worktree application flows

## Event Model

The canonical store should eventually be able to answer:

- which skills were attached to a turn
- which skills were consulted during a turn
- which sessions produced a skill proposal
- which proposal became an accepted skill patch
- which recurring errors led to a skill update

Suggested later event families:

- `skill.attached`
- `skill.viewed`
- `skill.proposed`
- `skill.applied`
- `skill.rejected`
- `improve.run.requested`
- `improve.run.finished`

These are later additions, not immediate v1 requirements.

## Safety And Review Rules

The safety bar should be:

- read-only skills first
- explicit review before applying any new skill or skill patch
- no silent direct edits to built-in harness prompt assets
- no direct “build a native tool” application path from the model

For native-tool proposals, the output should be:

- a design note
- or a patch draft
- or a scaffold in an isolated worktree

not an immediate mutation of the live runtime.

## Recommended Implementation Order

The right order is:

1. formalize the current markdown overlays as read-only first-class skills
2. add `skills_list` and `skill_view`
3. add operator-facing skill attachment and inspection surfaces
4. add proposal-backed skill authoring workflows
5. implement `/improve` in `bt-improve` as a session-review flow
6. later, allow constrained self-improving skill patches with review
7. only after that, consider tool-builder workflows

This ordering keeps Belltower:

- inspectable
- coherent
- reviewable
- aligned with its canonical event-first architecture

## Standard-Harness Test

This design should be considered successful only if:

- skills reduce prompt bloat rather than increasing it
- improvement artifacts are reviewable and event-backed
- the harness can explain why a skill exists and where it came from
- tools remain a smaller, higher-bar executable surface than skills
- self-improvement remains auditable rather than magical
