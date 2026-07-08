# Tool Policy

This document defines the intended shape of Belltower's tool-policy layer.

Read it with:

- [`./tool-system-and-normalization.md`](./tool-system-and-normalization.md)
- [`./memory-subsystem-spec.md`](./memory-subsystem-spec.md)
- [`./reactions-and-playbooks.md`](./reactions-and-playbooks.md)
- [`./hooks-and-lifecycle.md`](./hooks-and-lifecycle.md)
- [`../subsystems/tools-context-and-approvals.md`](../subsystems/tools-context-and-approvals.md)

It exists because tool behavior in Belltower is controlled by more than just:

- the tool schema
- the tool implementation
- the approval prompt

There is also a cross-cutting policy layer that determines:

- when a tool is visible
- when it is auto-approved
- when it requires human approval
- how it is classified for risk and mutability
- whether it may run concurrently
- whether it may interrupt or must defer
- what provenance, attachment, or boundedness rules apply

If this policy remains implicit, it will drift into:

- hardcoded runtime branches
- per-tool special cases
- subsystem-specific approval logic
- UI-only behavior that is not part of the harness contract

## Summary

Belltower should have an explicit Tool Policy layer.

This is a harness concern, not just an approvals concern.

Its job is to provide one place to express the execution and safety contract for tools across:

- built-in tools
- memory tools
- later workflow and delegation tools
- external tools where Belltower chooses to wrap or govern them

The important rule is:

- policy determines how a tool may run
- implementation determines what a tool does

Tool policy should be explicit in architecture and code, but it does not need to become a standalone crate immediately.

## Why this needs to be explicit

Without an explicit policy model, Belltower will tend toward several bad patterns:

- approvals become tool-name-specific special cases
- risk logic drifts separately from actual tool metadata
- memory, signals, and future workflow tools invent their own local rules
- concurrency and interruption semantics stay inconsistent
- the model-facing tool surface and the operator-facing trust model diverge

This matters because tool behavior is part of the harness contract.

For example:

- `shell` and `memory.write` are both writes, but they do not have the same risk profile
- `memory.fetch_context` is read-only, but still needs boundedness rules
- `ask` suspends for human input and therefore needs policy that is different from ordinary approvals
- future `delegate` or `branch` tools will need their own interrupt and routing policy

## What Tool Policy Owns

Tool policy should own the execution contract for a tool, including:

- visibility rules
- approval rules
- risk and mutability classification
- concurrency and interruption rules
- deferability and discovery behavior
- provenance or attachment requirements where relevant
- session-mode or scope gating

Tool policy should not own:

- the tool's user-facing description
- the tool's implementation logic
- provider-specific semantics
- prompt composition on its own

## Policy Dimensions

At minimum, Belltower tool policy should be able to express these dimensions.

### 1. Visibility

Whether the tool is:

- always visible
- mode-gated
- scope-gated
- conditionally discoverable
- hidden unless enabled by runtime policy

Examples:

- current `standard` vs `extended` mode differences
- future memory tools only visible when bindings exist
- deferred discovery through `catalogue`

### 2. Approval

Whether the tool is:

- always auto-approved
- auto-approved under bounded conditions
- approval-required
- denied by policy in the current context

This should be expressed structurally, not by scattered name checks.

### 3. Risk and Mutability

Whether the tool is:

- read-only
- operationally mutating
- durably mutating
- externally mutating

Examples:

- `read` is read-only
- `shell` may be operationally or externally mutating depending on arguments and environment
- `memory.register_resource` mutates Belltower-owned registry state
- `memory.write` mutates durable semantic memory

### 4. Concurrency

Whether the tool is safe to:

- run concurrently with other tools
- run only under serialization
- block other tool classes

This matters for file writes, memory writes, and future workflow or delegation tools.

### 5. Interrupt Behavior

Whether the tool:

- may interrupt current work
- must defer until the current turn phase completes
- suspends the turn for input or approval

Examples:

- `ask` suspends for input
- approval-required tools suspend until resolved
- some future workflow tools may require strict defer-until-safe behavior

### 6. Discovery and Prompt Cost

Whether the tool:

- is listed directly
- is conditionally discoverable
- is best reached through a meta-tool such as `catalogue`

This should remain a policy decision, not just a UI choice.

### 7. Provenance and Boundedness

Some tools need explicit policy beyond approval.

Examples:

- `memory.fetch_context` must obey bundle-size and citation rules
- future export tools may require structured provenance capture
- future dataset or workflow tools may require artifact registration before success is returned

## Relationship to Existing Tool Metadata

The current metadata direction is correct.

The existing shape already points toward this layer:

- `risk_class`
- `is_read_only`
- `is_concurrency_safe`
- `interrupt_behavior`
- `should_defer`
- `catalogue_tags`
- `display_group`

This metadata should be treated as the substrate of tool policy, not as a loose set of UI hints.

What still needs to be made more explicit is:

- how policy is evaluated
- which dimensions are mandatory for every canonical built-in
- which policy defaults exist for new tool families such as memory

## Relationship to Approvals

Approvals are one part of tool policy.

They are not the whole layer.

The split should stay clear:

- tool policy decides whether a tool is auto-approved, approval-required, deferred, or denied
- approval request and resolution persistence belong to the runtime/session path
- persisted approval request snapshots capture policy-relevant evidence at request time, while resolutions capture the human or policy decision
- any approval-required or user-input tool suspends the remaining tool-call batch
  immediately; later tool calls from the same model response must not execute
  until the approval or input path resumes the turn
- shell auto-approval is only valid for a single command plus bounded arguments;
  command composition, redirection, substitution, or other shell-control syntax
  must force an explicit approval even when the visible prefix matches an
  auto-approval pattern

This keeps approval prompts aligned with the broader execution contract instead of letting them become the only source of truth.

Approval snapshots must not be treated as executable input. The snapshot's
redacted argument preview is for operators, inspection, and export. Actual
execution after approval still uses the stored `tool.call.requested` arguments,
with the snapshot supplying the durable approval requirement, metadata, policy
surface, and request fingerprint.

## Relationship to Memory

Memory makes the need for explicit tool policy obvious.

Examples:

- `memory.search`
  - read-only
  - usually auto-approved
- `memory.view`
  - read-only
  - usually auto-approved
- `memory.fetch_context`
  - read-only but bounded
  - must satisfy policy for size, citations, and explicit attachment
- `memory.register_resource`
  - mutates Belltower-owned registry state
  - may be auto-approved only for certain trusted resource classes
- `memory.write`
  - durable semantic mutation
  - should remain approval-sensitive until stronger trust policy exists

The important lesson is:

- memory tools are not just “more built-ins”
- they are a cross-cutting policy test for the harness

## Relationship to Reactions and Hooks

Tool policy also matters outside model-selected tool calls.

Reactions may invoke deterministic pre-actions that resemble tool behavior:

- attach bounded memory
- register artifacts
- persist normalized signal payloads

Hooks may later do similar work around lifecycle boundaries.

The rule should remain:

- tool policy governs execution semantics even when the initiating path is not a model-chosen tool call

That does not mean every reaction pre-action or hook becomes a visible model tool.
It means the same risk, boundedness, and provenance standards should apply.

## Recommended Architectural Shape

In the architecture, Tool Policy should be treated as an explicit harness layer.

It does not need to be a separate crate in the first implementation.

Near term:

- keep policy data in `bt-core`
- keep policy evaluation in `bt-runtime`
- keep tool registration in `bt-tools`
- document policy requirements in the relevant subsystem docs

Later, if policy logic grows substantially, Belltower may extract a clearer internal module boundary or crate.

Do not extract it prematurely just because it is conceptually separate.

## Minimum Policy Contract For Canonical Tools

Every canonical built-in tool should eventually declare:

- risk classification
- mutability class
- approval behavior
- concurrency behavior
- interrupt behavior
- discovery behavior
- any special boundedness or provenance requirements

This should be true for:

- codebase tools
- planning tools
- inspection tools
- memory tools
- future workflow or delegation tools

## Current Implication

As Belltower grows, new tool families should not be added without a policy story.

That is especially true for:

- memory
- future delegation/session-graph tools
- workflow and reaction-facing tools
- artifact, export, and dataset tools

If a tool family cannot answer:

- when is it visible?
- when is it safe?
- when does it need approval?
- what must be logged or bounded?

then the harness design is not finished yet.
