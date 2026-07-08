# Datasets and Training Artifacts

This document defines how Belltower should support user-owned trajectory datasets, replay artifacts, and future model-improvement workflows.

It exists because Belltower's telemetry model is only fully differentiated if users can do more than inspect one session at a time.
They should be able to turn canonical sessions, branches, and related-session workflows into:

- reproducible trajectory datasets
- replay and evaluation corpora
- fine-tuning inputs
- future self-improvement and RL artifacts

This document should be read with:

- [`./overview.md`](./overview.md)
- [`./event-taxonomy-and-write-path.md`](./event-taxonomy-and-write-path.md)
- [`./subagents-and-session-graphs.md`](./subagents-and-session-graphs.md)
- [`../subsystems/telemetry-and-exports.md`](../subsystems/telemetry-and-exports.md)
- [`../planning/target-implementation-goals.md`](../planning/target-implementation-goals.md)

## Summary

The core rule is:

- the canonical session/event store remains the source of truth
- datasets are derived artifacts, not alternate runtime state

Belltower should eventually support dataset building directly from the canonical store so users can train or evaluate models on their own agent trajectories without first flattening the system into an ad hoc chat log.

That means Belltower should be able to derive, at minimum:

- full-fidelity trajectory artifacts
- compact fine-tuning artifacts
- replay/evaluation artifacts
- workflow-graph datasets spanning parent and child sessions

The architectural bet is that this should be built on top of Belltower-owned event/session history, not around it.

## Why This Matters

If self-improving or user-trained agent models become practical, the highest-leverage asset will not just be prompts and completions.
It will be high-quality, user-owned, tool-using, approval-aware trajectories with enough metadata to understand what happened and why.

Belltower is already positioned well for this because it owns:

- canonical structured session events
- raw provider chunks
- exact raw-to-structured correlation
- turn boundaries
- branch history
- related-session lineage
- usage and cost projection
- protocol-backed inspection surfaces

That is a much stronger substrate than a plain transcript export.

## Design Goals

Dataset support in Belltower should satisfy all of these:

1. derived from canonical stored history, never from transient UI state
2. reproducible from the same session store query
3. lineage-aware across branches and child sessions
4. redaction-aware and user-governed
5. useful for both humans and future autonomous self-analysis
6. compatible with lossy downstream formats without making those lossy formats canonical

## Non-Goals

This design does not require Belltower to claim any of the following today:

- capture of hidden chain-of-thought from providers that do not expose it
- a built-in RL trainer
- a built-in large-scale batch orchestration system
- automatic model fine-tuning in the runtime itself

It also should not force training concerns into `bt-agent`.
The agent loop remains a pure execution engine.
Dataset building should happen above the canonical store, not inside the turn loop.

## Canonical Data Boundary

The canonical data model is still:

- session metadata
- branch metadata
- event log
- raw provider chunks
- projections for messages, approvals, tool runs, branch heads, and cost summaries

Dataset artifacts are projections over that canonical data.

The important rule is:

- session history is the evidence
- datasets are curated views

If a dataset cannot be regenerated from canonical history plus explicit curation policy, it is not a trustworthy Belltower artifact.

## Dataset Layers

Belltower should eventually support at least four dataset layers.

### 1. Full-Fidelity Trajectory Artifact

This is the archival and research-grade form.

It should preserve:

- user/assistant/tool messages
- canonical event sequence
- turn boundaries
- tool calls and results
- approval requests and resolutions
- steer/cancel events
- provider/model identifiers
- token and cost metadata
- branch identity
- parent/child session lineage
- references to raw chunk ranges or raw chunk pages

This is the closest thing to "ground truth" for later replay or parser improvement.

### 2. Training Artifact

This is the model-training form.

It should be derived from the full-fidelity artifact using explicit policy:

- which turns to include
- how much context to keep
- whether tool traces stay explicit
- whether approvals are included as supervision
- what redactions are applied
- whether compaction summaries are retained or expanded

This artifact may be emitted in:

- Belltower-native JSONL
- ShareGPT-style projection
- provider- or trainer-specific formats later

The native format should remain richer than ShareGPT.

### 3. Replay / Evaluation Artifact

This form is optimized for:

- replaying an existing trajectory
- re-running a turn or session under a different model
- comparing tool decisions
- evaluating fidelity against the original trace

It should preserve deterministic boundaries and references:

- session and branch selection
- turn ordering
- event and raw-chunk anchors
- expected outputs and outcomes where available

### 4. Workflow-Graph Artifact

Belltower should go beyond flat session datasets.

Because child sessions and branch graphs are first-class in the design, the dataset layer should eventually support:

- one workflow root
- related sessions
- branch forks
- handoff summaries
- result-import edges

This is important because future autonomous work is unlikely to stay inside one transcript.

## Belltower-Native Trajectory Format

Belltower should define its own trajectory artifact format before optimizing for third-party trainers.

The native format should include:

- dataset metadata
- selection/redaction policy metadata
- one or more session/workflow entries
- event-range and turn-range references
- normalized message/tool/approval records
- usage/cost summaries
- lineage metadata

At minimum, one trajectory record should be able to answer:

- which session and branch did this come from?
- which turn(s) are included?
- which tools and approvals occurred?
- which provider/model generated it?
- what raw stream evidence exists behind it?
- what redaction policy was applied?

ShareGPT export should be treated as a lossy downstream projection, not the native artifact.

## Query and Curation Model

Dataset building should work over explicit selectors.

Examples:

- all sessions under one project root
- all turns from one date range
- all tool-using turns
- all approved shell executions
- all successful branches under one workflow root
- only child sessions of a given parent turn

Then filters should refine that selection:

- include only completed turns
- exclude auth failures
- exclude denied approvals
- include only specific providers/models/tools
- include or exclude compacted turns
- include or exclude raw chunks

This should be query-driven, not transcript-scraping.

## Redaction and Governance

If users train on their own traces, governance is mandatory.

Belltower should support redaction policies over canonical data before export:

- secret and credential removal
- environment-variable and auth-source masking
- path redaction or normalization
- URL/token query stripping
- optional PII-oriented redaction later

It should also support export policy controls such as:

- session marked exportable or non-exportable
- branch marked exportable or non-exportable
- event classes excluded from training artifacts

The important rule is:

- redaction is part of dataset construction
- redaction does not silently mutate canonical history

## Raw Chunk Role

Raw chunks are not ordinary training examples by default.
They are the evidence layer behind structured trajectories.

The dataset system should support:

- attaching raw chunk references to a trajectory
- optionally embedding raw chunk previews for debugging artifacts
- diffing raw chunks against structured projections during curation

For most fine-tuning corpora, the structured artifact will be primary.
For parser-debugging and evaluation corpora, raw chunk linkage is essential.

## Inspection and Agent Access

Belltower should expose dataset construction and inspection in two ways:

- human-facing commands and APIs
- agent-facing read-only tools over the same surfaces

Likely future human-facing commands:

- `/dataset`
- `/dataset build ...`
- `/dataset preview ...`
- `/dataset export ...`
- `/dataset replay ...`

Likely agent-facing tools:

- `inspect_dataset_candidates`
- `build_dataset_preview`
- `inspect_replay_artifact`

The same consistency rule still applies:

- dataset tooling should use canonical runtime/store APIs
- not hidden internal shortcuts

## Crate Boundary Recommendation

When implemented, dataset logic should live in a dedicated layer such as `bt-dataset`.

That crate should own:

- corpus selection
- dataset policy application
- redaction transforms
- native artifact emission
- downstream dataset format projections
- replay/eval artifact generation

It should not move into:

- `bt-agent`
- `bt-session`
- `bt-runtime`

Those crates should continue to own execution, persistence, and composition, not dataset curation.

## Relationship to Compaction

Compaction and dataset construction are related but not the same thing.

- compaction reduces active context for runtime execution
- dataset construction selects and transforms canonical history for offline use

The dataset layer may choose to:

- preserve compaction events as part of the trace
- expand around compacted regions when full-fidelity replay is needed
- emit compacted summaries as supervised artifacts in some formats

But compaction should not become a hidden dataset transform.

## Relationship to Subagents

This design depends on the branch-vs-child-session distinction.

- a branch is a speculative continuation inside one session
- a child session is delegated work with its own runtime state

That means future workflow datasets should preserve:

- branch edges inside a session
- parent/child edges across sessions
- result-import and handoff edges

This is one place where Belltower can go beyond most agent harnesses.

## Rollout Recommendation

This should land in phases.

### Phase 1. Design and Native Schema

- lock the native trajectory artifact shape
- define selectors, filters, and redaction policy structure
- define replay artifact structure

### Phase 2. Corpus Export

- export multiple sessions/branches through one dataset builder
- support project/date/provider/tool filters
- emit Belltower-native JSONL

### Phase 3. Governance and Redaction

- secret masking
- export policy controls
- normalized path/URL handling

### Phase 4. Replay and Evaluation

- build replay artifacts
- compare outputs across models
- attach trace/raw evidence

### Phase 5. Workflow Graph Datasets

- include child sessions and workflow lineage
- capture handoff/result edges

### Phase 6. Trainer Integrations

- richer ShareGPT projection
- trainer-specific adapters
- future RL hooks

## Human Checkpoints

When this subsystem is implemented, a human should be able to do all of the following:

1. select a set of sessions from one project
2. preview which turns would enter a dataset
3. confirm what redactions will be applied
4. export a native trajectory corpus
5. export a lossy fine-tuning projection such as ShareGPT
6. replay one selected trajectory under another model
7. inspect raw and structured evidence for any included turn

## Decision

Belltower should explicitly pursue user-owned trajectory datasets as a first-class consequence of its telemetry architecture.

But it should do so without collapsing the runtime into a training framework.

The correct layering is:

- canonical event/session store first
- dataset builder second
- training, replay, and self-improvement workflows on top of that
