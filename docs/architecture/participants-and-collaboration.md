# Participants and Collaboration

This document defines how Belltower should model collaborative sessions with multiple humans, agents, and external communication channels.

Read it with:

- [`./overview.md`](./overview.md)
- [`./signals-and-ingress.md`](./signals-and-ingress.md)
- [`./subagents-and-session-graphs.md`](./subagents-and-session-graphs.md)
- [`./event-taxonomy-and-write-path.md`](./event-taxonomy-and-write-path.md)

It exists because a general-purpose agent harness should not assume:

- one local human
- one agent
- one terminal
- one inbound path for all interaction

Belltower should be able to support:

- two or more humans working in the same session
- a human and an agent collaborating in one session
- remote human collaborators arriving through Slack, WhatsApp, or later email
- a parent agent coordinating with child agents
- mixed workflows where signals, human input, and delegated work all affect the same higher-level project

The important architectural rule is:

- collaboration is a harness concern, not a UI concern

The TUI, GUI, or remote chat surface may present collaboration differently, but the canonical model should live in Belltower itself.

## Summary

Belltower should have a first-class Participants and Collaboration subsystem.

That subsystem is responsible for:

- representing who is participating in a session or workflow
- preserving actor identity and provenance on canonical events
- managing memberships, roles, and permissions
- deciding how multiple participants share control of a session
- coordinating collaboration without collapsing distinct sessions into one transcript

This subsystem is adjacent to, but distinct from:

- Signals and ingress
  - how things arrive
- Subagents and session graphs
  - how related work is split across sessions

The clean relationship is:

- signals tell Belltower that something arrived
- collaboration tells Belltower who it belongs to and how it should affect the session
- session graphs tell Belltower whether the work belongs in this session or a related one

## Why this should be first-class

If collaboration is left to the UI or to ad hoc integration logic, Belltower loses exactly the things it is supposed to preserve:

- actor provenance
- approval attribution
- inspectable control flow
- replayable shared work
- clear boundaries between one shared session and multiple related sessions

This would make multiplayer or hybrid human-agent work look convenient at first, but ambiguous later.

Belltower should instead treat collaboration as a canonical runtime concern in the same way that it treats:

- turns
- approvals
- tool calls
- signals
- delegated sessions

## Core principles

### Collaboration is owned by the harness

Whether a participant appears in a local terminal, a web UI, Slack, or a future mobile client is a transport concern.

The harness should own:

- participant identity
- membership in sessions
- roles and permissions
- attribution on events
- turn coordination

### Signals and participants are different things

A signal tells Belltower that something happened.

A participant tells Belltower who is in the workflow and who should be credited, notified, or allowed to act.

These interact, but they should not be conflated.

Examples:

- a Slack message is a signal that resolves to a remote human participant
- a Semaphora action signal is a machine signal, not a participant
- a child session completion is an agent signal that may be attributable to a child agent participant

### Many participants can share a session, but control should stay explicit

A session may have many participants, but there should usually be:

- one active turn owner at a time
- explicit control-transfer rules
- explicit approval authority
- explicit interrupt and steering policy

Without that, shared sessions become concurrency soup.

### One shared session is not always the right model

Multiple humans can often collaborate in one session.

Multiple autonomous agents usually should not all write directly into one transcript.

For peer or delegated agent work, the preferred model is usually:

- separate related sessions
- explicit lineage between them
- typed durable messages passed across explicit same-lineage session and branch
  edges

The bounded subagent implementation follows this rule today for parents,
children, siblings, and deeper relatives in one lineage tree. It does not yet
implement the broader participant membership, role, permission, or turn-
arbitration subsystem described by the rest of this document.

### Provenance must survive rendering

A UI may choose to render remote input or agent results in a chat-like form.

That is fine as a projection.

The canonical event model must still preserve:

- the actor
- the participant kind
- the ingress source
- the session or related-session context
- the approval or control path that followed

## Core concepts

### Participant

A participant is an entity that can meaningfully take part in a Belltower workflow.

Examples:

- local human operator
- remote human collaborator
- primary agent for a session
- child agent or delegated worker
- system identity used for scheduled or administrative actions

At minimum, a participant should eventually have:

- `participant_id`
- `participant_kind`
- `display_name`
- optional stable external identities or handles
- metadata needed for operator inspection

### Membership

A membership links a participant to a session or higher-level workflow.

Membership is where collaboration policy lives.

At minimum, it should express:

- which session the participant belongs to
- what role the participant holds
- what permissions they have
- whether they are active, invited, suspended, or removed

### Actor

An actor is the specific participant credited on a canonical event.

This matters because one participant may produce many different events:

- prompt input
- steering
- approval
- rejection
- handoff
- remote reply

The event log should preserve actor identity directly rather than trying to infer it from message text.

### Role

Roles describe expected behavior within a session.

Examples:

- owner
- collaborator
- observer
- reviewer
- primary agent
- delegated agent
- system

Roles are not permissions by themselves, but they are the main ergonomic grouping.

### Permissions

Permissions describe what a participant may do.

Examples:

- prompt or reply
- steer the active turn
- approve tools or control actions
- interrupt work
- manage signal bindings for the session
- invite other participants
- spawn child sessions

Permissions should be explicit because collaborative scientific workflows often need asymmetric authority.

### Delivery handle

Some participants are reachable through external channels.

Examples:

- Slack thread or DM
- WhatsApp chat
- future email thread
- local TUI connection

Delivery handles belong to the collaboration layer, but they often originate from the signals layer.

## Participant kinds

The subsystem should at least distinguish:

- `local_human`
- `remote_human`
- `primary_agent`
- `delegated_agent`
- `external_agent`
- `system`

These are intentionally broader than ingress source kinds.

For example:

- both Slack and WhatsApp may resolve to `remote_human`
- both a child session and a future external worker could resolve to agent participants

## Session model

Collaborative state should be anchored to sessions.

At minimum, Belltower should be able to answer:

- who belongs to this session
- which participant produced this event
- which participants are allowed to steer, approve, or interrupt
- which remote handles map into this session
- whether a workflow should continue in this session or a related child session

### Shared session

Use one shared session when:

- humans are collaborating on the same live thread of work
- the work remains sequential enough to share one turn queue
- one transcript is still the right unit of inspection

Examples:

- a scientist and a collaborator discussing the same draft
- a scientist and a primary agent iterating on one paper section
- multiple remote human collaborators contributing into one routed project session

### Related sessions

Use related sessions when:

- delegated work should proceed independently
- approvals should be isolated
- the work has its own budget, tools, or worktree
- the parent mainly wants a result, not another long transcript

Examples:

- a child agent doing literature review
- a peer agent running an analysis branch
- a reviewer agent inspecting a draft in parallel

This aligns directly with [`./subagents-and-session-graphs.md`](./subagents-and-session-graphs.md).

## Turn ownership and arbitration

The most important operational rule is:

- many participants may belong to a session, but turn ownership should stay explicit

At minimum, the collaboration subsystem should eventually support:

- one active turn owner
- a queue for additional participant input
- explicit steering or interrupt requests
- clear approval authority
- operator-visible handoff or ownership transfer

This allows Belltower to support multiplayer without pretending that fully concurrent transcript mutation is easy to reason about.

### Practical rules

- multiple participants may submit input while a turn is running
- those inputs should enter a queue or inbox unless policy says they interrupt
- interrupts should be explicit and attributable
- approval rights should be inspectable per participant
- agent participants should not silently inherit broad human permissions by default

## Relationship to signals

Signals and collaboration should interoperate directly.

The normal flow is:

1. a signal arrives through the Signals subsystem
2. routing decides which session or inbox it belongs to
3. participant resolution decides who the signal corresponds to, if anyone
4. collaboration policy decides how it should affect the session
5. the result becomes a canonical session event, inbox item, or control action

Examples:

- a Slack message arrives
  - it is a signal first
  - then it resolves to a remote human participant in a shared session
- a Semaphora action signal arrives
  - it remains a machine signal
  - it may notify or steer participants, but it is not itself a participant
- a child agent publishes a completion result
  - it is an agent signal
  - it may also be attributable to a delegated agent participant

This is why Signals and Collaboration are separate subsystems that must be designed together.

## User registration and onboarding

Users should be able to register participants and collaboration surfaces intentionally.

The collaboration layer should not depend on hidden UI state.

### Participant registration

Users should eventually have first-class surfaces to:

- add a collaborator to a session
- invite a remote participant
- link a remote handle to a participant
- grant or revoke permissions
- list current participants and roles

Conceptually, this should look like:

- `bt participant add`
- `bt participant invite`
- `bt participant link-handle`
- `bt participant list`
- `bt participant remove`

The exact command shape can change, but the underlying capability should be first-class.

### Session sharing

For casual collaboration, Belltower should support a share flow such as:

- `belltower share`

That flow should issue a short-lived invite or session token and attach the resulting participant to the session explicitly.

This keeps multiplayer practical without turning collaboration into a deployment-specific hack.

### Registering signal-backed collaborators

Some participants arrive through signal-producing systems.

Examples:

- a Slack collaborator joining a shared session
- a WhatsApp conversation mapped to a project session

In those cases, the user should register both:

- the signal binding
- the participant or participant mapping

This is another reason signals and collaboration must remain separate but interoperable.

## Example patterns

### Two humans in one session

- local operator and remote collaborator are both participants
- both can submit input
- one active turn owner exists at a time
- approvals may remain restricted to the owner

### Scientist, primary agent, and Semaphora

- scientist and primary agent are participants
- Semaphora is not a participant
- Semaphora signals create inbox items, review work, or queued actions in the session

### Parent agent with child agent

- parent and child are separate sessions
- child may still be represented as a delegated-agent participant in the higher-level workflow view
- canonical control and telemetry still stay session-scoped

### Slack-backed project collaboration

- Slack is a signal producer
- remote humans are participants
- routing maps thread messages into the right session
- delivery handles allow structured replies back into the same thread

## Canonical event implications

The canonical event model should eventually make collaboration explicit.

That likely implies event and metadata coverage for:

- participant registration or linking
- session membership changes
- invite issuance and revocation
- turn ownership changes
- approval attribution
- remote reply attribution

The exact taxonomy can evolve, but the rule should remain:

- actor and participant provenance must be canonical, not inferred from rendered chat text

## Recommended rollout

Collaboration should land in phases.

### Phase 1. Identity and membership

- first-class participants
- session memberships
- actor attribution on events

### Phase 2. Permissions and sharing

- roles and permission bundles
- invite and share flows
- remote handle mapping

### Phase 3. Turn arbitration

- explicit turn ownership
- queued participant input
- controlled interrupt and steer semantics

### Phase 4. Cross-session workflow views

- unified inspection across shared sessions and delegated child sessions
- collaboration-aware routing and review surfaces

## Strategic value

Most agent harnesses either:

- assume one human and one agent in one transcript
- treat collaboration as a messaging integration detail
- or blur delegated work into chat history

Belltower should instead make collaborative scientific work:

- inspectable
- attributable
- auditable
- replayable

That is the path to real shared human-agent workflows rather than a thin chat wrapper over tools.
