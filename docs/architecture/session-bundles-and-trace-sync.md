# Session Bundles And Trace Sync

This document defines the target architecture for portable Belltower session
artifacts, offline validation, structural diffing, and append-only remote sync.

The goal is narrow: make a Belltower session portable enough that another
operator can validate it, inspect it, import it, fork from any node, continue the
work, and later publish selected evidence. This is not a Git replacement and it
is not a second runtime store.

## Principles

- `bt-session` remains the canonical owner of session evidence.
- A `session.bt` bundle is portable evidence, not a projection cache.
- Projections are derived at runtime from canonical evidence.
- Remote sync distributes verified evidence; it does not become semantic truth.
- Events and payloads are immutable. Branch heads are pointers.
- Portable identity is content-derived. Local SQLite `seq_id` values, local raw
  chunk row ids, and local file paths are debugging aids, not portable identity.
- Complexity should be paid for by validation, replay, continuation, sharing, or
  scientific artifact provenance.

## Non-Goals

- No merge semantics like Git.
- No canonical transcript, OTLP, HTML, ShareGPT, or search projection inside the
  core bundle.
- No ORCID, citation, or public publication semantics in the first bundle
  implementation.
- No remote registry requirement for local export, validation, import, or diff.
- No foreign trace format as Belltower's source of truth.
- No backwards-compatibility promise for the existing `/export legacy-bundle` JSON
  response. Belltower is still in active development; the manifest-owned
  `session.bt` format should replace stale portable/export shapes rather than
  preserving aliases that obscure the canonical record.

## Core Data Model

Keep the model small. Export, import, sync, subagent handoff, multiplayer, and
artifact materialization should reuse these same concepts instead of each
creating its own identity system.

### Portable Identity

The SQLite store can keep local ordering and row identifiers. A portable bundle
must not depend on those values.

Use three identities deliberately:

- `event_id`: the stable Belltower event id generated when the event is
  committed. Import should preserve it unless an explicit rehydration mode says
  otherwise.
- `store_seq_id`: the local SQLite sequence id. This is useful for local paging
  and debugging, but it can change when a bundle is imported elsewhere.
- `event_hash`: the portable content identity used by bundles, validation,
  diff, sync, and external parent refs.

Portable hashes use one concrete representation in the first implementation:
`sha256:<lowercase-hex>`. If Belltower later needs a different algorithm, that
is a schema-versioned bundle change rather than an optional per-field variant.
In Rust this should be a small newtype rather than a free-form `String`; in JSON
it serializes as the canonical string form.

`event_hash` is computed over canonical JSON bytes for the portable event
record. Canonical event bytes exclude local-only fields such as imported-store
`seq_id`, raw chunk row ids, and bundle-local file paths. They include stable
event identity, session/branch/turn refs, event kind, timestamp, payload, and
normalized content refs.

Ordering inside a bundle is represented by bundle-local event ordinals and by
hash continuity. Portable `session.bt` records do not export SQLite
`store_seq_id` values. Validators and diffs use bundle order plus event hashes,
not imported SQLite sequence numbers.

### `SessionNodeRef`

`SessionNodeRef` is the universal pointer to a point in session history.

It is used for:

- branch continuation
- imported session parent refs
- artifact evidence boundaries
- subagent handoff refs
- remote branch heads
- diff common ancestors

Conceptual shape:

```rust
struct Hash(String); // `sha256:<lowercase-hex>`

struct SessionNodeRef {
    bundle_hash: Option<Hash>,
    session_id: SessionId,
    branch_id: BranchId,
    bundle_event_ordinal: u64,
    store_seq_id: Option<i64>,
    event_id: EventId,
    event_hash: Hash,
}
```

`bundle_event_ordinal` is the bundle-local order. `event_hash` is the portable
identity. `store_seq_id` exists for local in-store pointers, but bundle-owned
node refs set it to `None`; exported bundles must not depend on an importing or
exporting SQLite sequence number. A bundle importer may rewrite local database
sequence numbers, but it must preserve the external node reference.

### `ContentRef`

`ContentRef` is the universal pointer to bytes outside the event envelope.

It is used for:

- raw provider chunks
- large tool outputs
- file artifacts
- patches
- generated reports, figures, datasets, and papers
- redacted payload replacements

Conceptual shape:

```rust
struct ContentRef {
    hash: Hash, // `sha256:<lowercase-hex>`
    media_type: Option<String>,
    size_bytes: u64,
    path: Option<String>,
}
```

The content hash is the identity. Paths are bundle-local display or lookup
hints, not identity.

### `EventRange`

`EventRange` is the validation, diff, and sync unit.

Conceptual shape:

```rust
struct EventRange {
    branch_id: BranchId,
    ordinal_start: u64,
    ordinal_end: u64,
    first_event_hash: Hash,
    last_event_hash: Hash,
    exported_store_seq_start: Option<i64>, // always None in portable bundles
    exported_store_seq_end: Option<i64>,   // always None in portable bundles
}
```

Remote sync pushes event ranges plus content refs. Diff compares event ranges
and branch heads. Validation checks range ordering, continuity, hashes, branch
refs, and payload refs.

### `BundleArtifactRef`

`BundleArtifactRef` describes a work product materialized from a specific
evidence boundary.

This name is intentional. `bt-core` already has an `ArtifactRef` used by tool
operation metadata. The bundle primitive either needs to evolve that existing
type deliberately or remain separately named so the implementation does not
collapse tool-result metadata and scientific artifact provenance by accident.

Conceptual shape:

```rust
struct BundleArtifactRef {
    artifact_id: String,
    kind: ArtifactKind,
    source_node: SessionNodeRef,
    content: ContentRef,
}
```

Examples:

- a paper generated from all evidence up to a node
- a patch set produced by a coding session
- a figure produced by a data-analysis session
- a report derived from a subagent result

The source node matters. If later session history changes, the artifact still
points to the evidence boundary that produced it.

### `SessionBundleManifest`

The manifest describes what a bundle contains and how it should be verified.

Conceptual shape:

```rust
struct SessionBundleManifest {
    schema_version: u32,
    bundle_hash: Hash,
    session: SessionRecord,
    branches: Vec<BranchManifest>,
    event_ranges: Vec<EventRange>,
    producer: ProducerInfo,
    redaction: RedactionPolicy,
    artifact_mode: SessionBundleArtifactMode,
    contents: Vec<ContentRef>,
    artifacts: Vec<BundleArtifactRef>,
}
```

The manifest should be compact enough to inspect manually. It owns the
`SessionRecord` because local import needs the project root, connection/model
selection, settings revision, tool mode, parent session refs, and creation
metadata without guessing from events. It should not embed large payloads or
derived projections.

`artifact_mode` records whether the bundle is trace-only, trace plus patch
artifacts, or trace plus explicitly selected artifacts. The mode is descriptive
and verifier-visible; artifact bytes are still addressed through `ContentRef`
and `BundleArtifactRef`.

## `session.bt` Bundle Shape

`session.bt` may be stored as an archive for transport and as an unpacked
directory for debugging. The canonical shape is:

```text
manifest.json
events.jsonl
raw_chunk_refs.jsonl
raw_chunks/
payloads/
artifacts/
checksums.json
```

Required content:

- `manifest.json`: bundle schema, session record, producer, event ranges,
  branch heads, external parent refs, content refs, artifact refs, artifact
  mode, and redaction mode
- `events.jsonl`: `SessionBundleEventRecord` rows containing
  `bundle_event_ordinal`, declared `event_kind`, `event_hash`, and the canonical
  portable event envelope
- `raw_chunk_refs.jsonl`: mapping from bundle-local raw chunk ordinals and
  event/content refs to deterministic content refs
- `raw_chunks/`: content-addressed raw provider stream chunks
- `payloads/`: large tool outputs or structured payloads referenced from events
- `artifacts/`: optional work products such as patches, reports, figures, or
  papers
- `checksums.json`: hash tree over events, chunks, payloads, and artifacts.
  `manifest.json` stores the resulting `bundle_hash`; the manifest itself is
  validated for internal consistency but is not included in the checksum tree,
  avoiding a circular manifest-hashes-itself rule.

Excluded from the core bundle:

- transcript view
- TUI history view
- OTLP/OpenInference spans
- ShareGPT or training trajectories
- HTML export
- search index
- branch summaries that can be regenerated from canonical events

Those may be emitted as separate exports or optional caches, but validation and
import must be able to ignore them.

### Legacy `/export legacy-bundle` Surface

The current `/sessions/{session_id}/export/legacy-bundle` endpoint returns a legacy
JSON object with session records, branches, default-branch messages, events, and
raw chunks. That response is not the portable `session.bt` bundle described
here, and it should not be extended.

Because Belltower is still actively developing, the portable bundle surface has
cut over to `session.bt` instead of maintaining legacy bundle compatibility. The
legacy JSON shape is a legacy/debug snapshot, not a sharing or review contract.
Tests that assert projection fields such as `messages` or local row ids inside a
portable bundle should be rewritten, not preserved.

Archive or directory-style bundle transport should not be forced through the
existing `SessionExportResponse { content: String }` shape. Text projections can
keep using that response; portable bundle operations need file/archive-aware
interfaces.

## Raw Chunk Identity

The current SQLite store may use row ids or local chunk ids internally. Portable
bundles must not depend on those row ids.

The bundle should carry a two-way mapping:

```text
raw_chunk_refs.jsonl:
  chunk_ordinal
  event_id
  provider
  llm_call_ordinal
  stream
  content
```

Raw chunk content refs are part of the first bundle schema/export slice, not a
later migration. Portable event payloads replace local raw chunk row ids with
`raw_chunk_content_ref = sha256:...`, and portable replay resolves through
`event_id` plus `content_hash`. That pair is intentional: two events may contain
identical provider bytes, so a bundle must not collapse raw chunks by content
hash alone. Multiple event-level refs may point at the same `chunk_ordinal`; for
example, a raw persistence event and the normalized completion chunk event can
both refer to one stored provider chunk. Import recreates one local raw chunk per
bundle `chunk_ordinal` and denormalizes each event ref to that imported local row
inside the destination SQLite store.

## Branching Across Bundles

Continuation across bundles should reuse Belltower's branch model with explicit
external parent refs.

When a user imports a bundle and continues from a node:

```text
new local session or branch
  parent_external_ref = SessionNodeRef from imported bundle
```

This lets multiple local sessions share ancestry without flattening them into
one log. It also gives subagents and multiplayer sessions the same lineage
primitive.

The first local implementation uses `belltower session continue <bundle>
[--event-hash sha256:...]`. If the source bundle is not already present in the
local store, the command imports it first. It then creates a new default branch
whose `parent_branch_id` and `parent_event_id` point at the selected imported
node. The resulting `branch.created` event carries the external
`SessionNodeRef` in the `belltower.session.parent_external_ref` attribute, so
bundle ancestry remains portable without adding a separate lineage table.
If the source session already exists locally, continue recomputes the selected
parent event hash from the local store and rejects the operation when it differs
from the bundle's `event_hash`. Matching `event_id` alone is not sufficient.

The same branch seam is used for local rewind. `CreateBranchRequest` may carry
`from_event_id`; when present, the runtime creates the child branch with that
event as `parent_event_id`. Reads and context preparation replay the parent
lineage only through that boundary. No existing raw events are edited or
deleted.

## Artifact Materialization

Any node can serve as an evidence boundary for a materialized artifact.

Example:

```text
source_node = SessionNodeRef(...)
artifact = paper.md
artifact_hash = sha256(...)
```

The artifact can be created later than the node it references. The important
claim is: "this paper used evidence up to and including this node."

This supports:

- papers
- figures
- datasets
- report snapshots
- patches
- replication packages

The first implementation is explicit and local-root constrained:

- `belltower session export <session-id> --out <dir>` creates a trace-only
  bundle.
- `belltower session export <session-id> --out <dir> --artifact <path>` creates
  a trace-plus-artifacts bundle. Relative artifact paths resolve against the
  session project root, and artifact files must remain inside that root after
  canonicalization.
- `belltower session export <session-id> --out <dir> --mode trace-plus-patches`
  adds a `git diff --binary --no-color HEAD` patch artifact when the session
  project root is a Git checkout with local changes. Explicit `--artifact`
  inputs in this mode are classified as patch artifacts.

This keeps artifact selection operator-owned and auditable. Belltower does not
scan the project tree for artifacts or infer publication contents.

## Validation

`belltower session validate <bundle>` should be a first-class local command
before remote sync exists.

Validation checks:

- manifest schema and version
- closed inventory: every file is either manifest/checksum metadata or is
  referenced by the checksum tree, and every checksum entry is required by the
  manifest or bundle shape
- bundle-local event ordering and contiguous ranges
- event hashes and hash-chain/hash-tree membership
- branch heads, branch ordinals, and parent refs resolve to the exact event
  tuple they claim
- raw chunk refs resolve to content blobs and to the branch/event that emitted
  them; every event-level raw chunk content ref must have a matching
  `(event_id, content_hash)` raw chunk ref
- payload and artifact refs resolve
- content hashes and byte counts match
- no dangling cross-bundle refs unless explicitly allowed as external refs

Validation output should be readable and machine-parseable. External readers
should be able to validate a bundle offline before they import, pull, or publish
anything.

Import is atomic. If validation succeeds but event denormalization or append
fails after the transaction starts, the destination store must not retain a
partial session, partial branches, or imported raw chunks.

## Diff

`belltower session diff <a> <b>` should compare bundles structurally.

Diff modes:

- same lineage: added event ranges, branch-head movement, new payloads, new
  artifacts
- forks: common ancestor node plus divergent event ranges
- redaction preview: full bundle vs publication bundle
- replication review: original bundle vs replicated attempt

Diff should not be only text diff. It should report:

- event ranges added or missing
- branch heads advanced or forked
- raw chunks added or missing
- tool calls and tool results added or changed
- artifact refs added, changed, or removed
- redactions applied

The first local diff implementation classifies bundles by portable event hashes
and structural inventory rather than by SQLite sequence numbers or raw chunk row
ids. It reports equivalent bundles, same-lineage updates, fork divergence,
different lineage, redaction differences, and non-event structural differences
such as branch/content/artifact inventory drift.

## Remote Sync

Remote sync is append-only replication of verified session evidence.

The first private implementation is intentionally filesystem-backed:
`belltower session push <bundle> --remote <dir>` validates a local bundle, copies
it into a content-addressed remote directory, and advances the selected branch
head only if the compare-and-swap check passes. `belltower session pull --remote
<dir> --session <id> --out <dir>` copies the selected remote bundle back to a
local directory and validates it offline. Pull does not import or continue the
session; those remain explicit `session import` or `session continue` steps.
This keeps remote transport separate from session semantics while giving the
future HTTP registry a concrete state model to expose.

Conceptual remote objects:

- `SessionRecord`
- `BranchHead`
- `EventRange`
- `PayloadBlob`
- `BundleManifest`

Push semantics:

```text
push(branch, expected_head = A, new_head = B)
```

The remote updates the branch head only if its current head is still `A`.
If the head changed, the push rejects with a conflict and the client must pull,
validate, inspect or diff, then retry or create a continuation branch.

This is compare-and-swap semantics. It prevents silent overwrite in
multi-actor scenarios without introducing Git-like merge behavior.
The filesystem-backed implementation also serializes head updates with a small
remote state lock so two local push processes cannot both pass the same CAS
check and race while writing the head file. A future HTTP registry should expose
the same atomic `advance from A to B` operation instead of relying on client-side
locking.

Pull semantics:

- download the referenced bundle or event ranges
- verify hashes and refs
- import into a local namespace
- optionally continue from a selected `SessionNodeRef`

Pull must not mutate the current session without an explicit continue or fork
operation.

## Operation Audit Records

Export, validate, import, diff, push, pull, share, and publish are operator-
visible operations. When they affect operator-visible truth or durable workflow
state, they should record a compact operation summary through the same canonical
operator-command/tool-operation path used elsewhere in Belltower.

Record summaries, not exported content. A summary should include the relevant
fields for the operation:

- operation name and actor
- session id, branch id, and source `SessionNodeRef` or `EventRange`
- manifest hash or bundle hash
- validation status
- redaction policy
- output location or remote ref
- for sync: expected head, new head, and conflict status

Exporting a node must not mutate the evidence boundary being exported. If the
export operation itself is recorded, that record belongs to a later node in the
same session or to a separate audit session.

## Sync, Share, Publish

Keep these concepts separate.

### Sync

Private durability and backup.

- can happen in the backend after committed events
- may be debounce-based or range-based
- should not require the agent to call a tool
- should sync only canonical committed evidence

### Share

Operational distribution of a validated bundle or remote ref to another human,
agent, or session.

- may be initiated by the operator or, with policy, by an agent tool
- should remain append-only
- should preserve lineage refs

### Publish

Scientific/public artifact creation.

- requires redaction policy
- requires validation
- may add signatures, ORCID identity claims, citation metadata, or RO-Crate
  metadata later
- should freeze a verified node/range as an artifact

ORCID should identify an accountable human for a published artifact. It should
not become the runtime participant id, approval authority, or canonical session
truth.

## Subagents And Multiplayer

The same bundle primitives support current child-session evidence and should
support future multiplayer workflows without changing the canonical event log.

For subagents:

- parent emits a spawn or handoff event
- child writes its own session log
- parent and child exchange typed durable related-session messages
- each exported bundle preserves that session's mailbox event copy
- richer child artifacts or bundle refs may be returned without flattening the child log
- parent does not flatten the child history into its own log

For multiplayer:

- server-sequenced append-only events remain canonical
- concurrent offline work creates forks or continuation branches
- participant identity belongs in event provenance
- branch mechanics stay independent of identity provider details

## Crate Ownership

- `bt-core`: shared IDs, event payloads, bundle DTO primitives if they are used
  by multiple crates
- `bt-session`: bundle export, validation, import, raw-ref mapping,
  deterministic replay/reducer logic
- `bt-protocol`: public route DTOs for session bundle operations
- `bt-server`: thin transport for validate/import/export/push/pull routes
- `bt-client`: typed client helpers
- `bt-otel`: derived OTLP/OpenInference projections only
- `bt-tools`: optional agent-facing sync/share tools, policy-gated
- `belltower`: CLI commands such as `session validate`, `session diff`,
  `session push`, and `session pull`

## Implementation Order

1. Define the bundle schema, portable identity rules, concrete hash
   representation, and canonical type names. Complete for the local
   `session.bt` foundation.
2. Remove or explicitly rename the legacy JSON bundle surface so it cannot be
   confused with `session.bt`. The legacy JSON surface remains debug-only and
   is not the portable sharing format.
3. Implement local `session.bt` export with deterministic `ContentRef`s and
   `raw_chunk_refs.jsonl`. Complete for trace-only and explicit artifact modes.
4. Implement `belltower session validate <bundle>` with closed inventory
   validation. Complete.
5. Implement local `belltower session import <bundle>` with atomic import and
   re-export fidelity. Complete.
6. Implement `belltower session diff <a> <b>` and local continuation from
   `SessionNodeRef`, including parent-hash verification for already-imported
   sessions. Complete.
7. Implement private remote push/pull with compare-and-swap branch-head updates.
   Complete for filesystem-backed sync.
8. Add artifact modes such as trace-only, trace-plus-patches, and
   trace-plus-artifacts. The first shipped form is explicit artifact capture
   plus optional Git patch capture; no automatic workspace scan.
9. Add publish/register later with redaction, signatures, ORCID, citation
   metadata, and archival package metadata.

## Acceptance Checks

- export, validate, import, and re-export preserve canonical events, branch
  lineage, raw refs, and artifact refs
- imported sessions receive fresh local `store_seq_id` and raw chunk row ids
  while preserving `event_id`, `event_hash`, content refs, and external
  `SessionNodeRef`s
- validation rejects malformed manifests, bad hashes, missing payloads,
  non-contiguous ranges, mismatched branch-head tuples, dangling refs, unknown
  files, unreferenced checksum entries, and missing checksum paths
- diff identifies same-lineage updates, fork divergence, and redaction effects
- continuation from a mid-session node reconstructs context only up to that node
- continuation against an already-imported session rejects a matching event id
  when the recomputed parent event hash differs from the bundle
- failed imports roll back without leaving a partial session
- remote push rejects stale expected heads
- remote push rejects concurrent head-update races
- pull imports without mutating the current session unless explicitly continued
- derived projections can be regenerated from the bundle and are not required
  for validation
