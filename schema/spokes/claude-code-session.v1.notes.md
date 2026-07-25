# Claude Code Session Format — Empirical Notes (v1)

Companion to [`claude-code-session.v1.schema.json`](./claude-code-session.v1.schema.json).
The schema is the defined truth the Belltower ingress/egress adapter is built
against; these notes record where that truth came from, the linkage invariants
an importer relies on, and what was deliberately classified as noise.

All examples below are synthesized (neutral content, zeroed ids). No real
transcript text, non-generic paths, or personal data appear in this file or in
the schema.

## 1. Provenance

Surveyed store: `~/.claude/projects/` on one operator machine, snapshot
2026-07-25, plus two controlled fresh captures the same day.

| corpus dimension | value |
|---|---|
| project directories | 23 (incl. 5 `ssh-*` remote slugs and several scratchpad slugs) |
| main session files (`<sessionId>.jsonl`) | 30 |
| sidecar subagent transcripts (`agent-*.jsonl`) | 112 (incl. 13 under `subagents/workflows/wf_*/`) |
| sidecar agent meta files (`agent-*.meta.json`) | 111 |
| total JSONL lines | 36,279 (one file is a live session; counts are a snapshot) |
| unparseable / non-object lines | 0 / 0 |
| `version` field range | 2.1.111 … 2.1.219 |
| timestamp range | 2026-04-24 … 2026-07-25 |
| fresh-capture CLI | `claude --version` → `2.1.118 (Claude Code)` (sandboxed bundled binary), 3 sessions, 40 lines: one Read-tool run, one blocked-tool run, one no-tool run |

Version-to-date mapping is **not monotonic**: sandboxed/SDK environments write
with a pinned bundled CLI (2.1.118 lines dated July next to 2.1.217 desktop
lines). `version` identifies the writer, not the era.

Observed versions per era: 2.1.111 (Apr), 2.1.119–2.1.128 (Apr–May),
2.1.165–2.1.170 (Jun), 2.1.197–2.1.219 (Jul), 2.1.118 (pinned sandbox CLI,
Jul). Field drift across this range was additive only; no observed field
changed JSON type between versions (unions in the schema are within-version
variation).

## 2. Store layout

```
~/.claude/projects/<cwd-slug>/
  <sessionId>.jsonl                 # main transcript (this schema, line by line)
  <sessionId>/                      # per-session sidecar dir (optional)
    subagents/
      agent-<agentId>.jsonl         # subagent transcript (same line schema)
      agent-<agentId>.meta.json     # spawn metadata ($defs/agentMetaFile)
      workflows/wf_<id>/
        agent-<agentId>.jsonl       # workflow subagent (adds started/result lines)
        agent-<agentId>.meta.json   # {"agentType": "workflow-subagent"} only
    tool-results/                   # overflow storage for large tool outputs
      toolu_*.json                  #   JSON content-array of a tool_result
      <taskId>.txt                  #   raw text of a background task output
      webfetch-*.pdf                #   fetched binaries
  sessions-index.json               # project-level session listing (bookkeeping)
  memory/                           # auto-memory markdown, not session data
```

`<cwd-slug>` is the working directory with `/` and `.` replaced by `-`
(e.g. `-Users-alice-projects-demo`); remote sessions use `ssh-<uuid>` slugs.
Only the `*.jsonl` files and `agent-*.meta.json` are schema'd. The
`tool-results/` files, `sessions-index.json`, and `memory/` are out of scope
for the line schema (see §6).

## 3. Line types observed (corpus counts, snapshot)

| `type` | count | class |
|---|---|---|
| `assistant` | 18,881 | conversation chain |
| `user` | 9,685 | conversation chain |
| `queue-operation` | 1,972 | bookkeeping (no uuid) |
| `last-prompt` | 1,250 | bookkeeping (no uuid) |
| `ai-title` | 1,228 | bookkeeping (no uuid) |
| `attachment` | 1,162 | chained context-injection bookkeeping |
| `mode` | 755 | bookkeeping (no uuid) |
| `custom-title` | 652 | bookkeeping (no uuid) |
| `system` | 536 | chained harness events (5 subtypes) |
| `pr-link` | 89 | bookkeeping |
| `frame-link` | 8 | bookkeeping |
| `started` | 5 | workflow sidecars only |
| `result` | 4 | workflow sidecars only |
| `file-history-snapshot` | 3 | bookkeeping |
| `permission-mode` | 1 | bookkeeping |

The historically documented `summary` line type was **never observed** in this
corpus (36k lines, Apr–Jul 2026) and is therefore not in the schema. If other
machines' stores contain it, that is a schema revision, not a validator bug.

Subagent transcripts contain only `user`/`assistant`/`attachment` (all with
`isSidechain: true` and `agentId`) plus, under `workflows/`, `started`/`result`.
Zero `isSidechain: true` lines occur in main transcripts: in this era the
sidechain lives entirely in sidecar files.

## 4. Linkage invariants (verified, importer-load-bearing)

1. **File order is authoritative.** Bookkeeping rows interleave anywhere
   (an `ai-title` may precede the first `user` line or follow the last).
   Timestamps tie and interleave; never sort by them.
2. **uuid / parentUuid chain.** `uuid` is unique within a file (30,372
   records, 0 duplicates). `parentUuid` refers to an *earlier* record in the
   *same file*; chain roots have `parentUuid: null` (first user prompt, and
   `compact_boundary` system rows). Verified 30,221/30,222; the single
   exception (1 in 30k) was a `tool_result` user row whose parent assistant
   line was evidently rewritten on resume — its uuid is gone but the tool
   linkage still resolves. Importers must treat parentUuid resolution as
   best-effort and terminalize honestly, not crash.
   `assistant.parentUuid` was never null in 18,881 samples.
3. **One API response = N consecutive assistant lines.** Each assistant line
   carries exactly one content block; lines of the same response share
   `message.id` (`msg_*`) and top-level `requestId` (`req_*`) and chain
   through parentUuid in block order (thinking → text → tool_use).
   `message.usage` is duplicated on every line of the response — count usage
   once per `message.id`, not per line.
4. **Tool call linkage.** `tool_use.id` (`toolu_*`) on an assistant line is
   answered by a later user line whose `message.content[]` holds a
   `tool_result` with `tool_use_id` equal to it. Verified: 8,457/8,457
   tool_results matched within their transcript; 3 tool_uses unanswered
   (operator interrupts) — the dangling-structure case the import validator
   must terminalize. The answering user line also carries
   `sourceToolAssistantUUID` (uuid of the assistant line holding the
   tool_use) and the harness-internal `toolUseResult` payload.
5. **sessionId constancy.** Every line of a main file and of all its sidecar
   transcripts carries the same `sessionId`, equal to the main file's
   basename (verified across all 142 files; zero files with >1 sessionId).
   A subagent transcript is identified by `agentId`, not by a sessionId of
   its own.
6. **Sidecar linkage.** `agent-<agentId>.meta.json` sits beside
   `agent-<agentId>.jsonl`; every line of the transcript that has `agentId`
   matches the filename id (111/111 files). `meta.toolUseId` is the `toolu_*`
   id of the spawning Task-tool `tool_use` in the parent transcript (the main
   file, or another agent's transcript for nested spawns) — resolved
   103/103 where present. Nesting: `parentAgentId` + `spawnDepth ≥ 2`
   (41 nested metas observed). Workflow subagents
   (`agentType: "workflow-subagent"`) have **no** `toolUseId`; their
   transcripts instead begin/end with `started`/`result` lines pairing by
   `key` (`v2:` + 64 hex) and `agentId`.
7. **Compaction.** A `system` row with `subtype: "compact_boundary"`,
   `parentUuid: null`, `logicalParentUuid` = last pre-compaction uuid, and
   `compactMetadata {trigger, preTokens, postTokens, durationMs, ...}` starts
   a fresh chain; the next `user` row (`isCompactSummary: true`,
   `isVisibleInTranscriptOnly: true`) parents onto the boundary row and
   carries the synthetic continuation prompt. The pre-compaction chain stays
   in the file; `logicalParentUuid` is the only bridge (6 instances observed).

Id shapes: record/session/prompt ids are RFC-4122 UUIDs; `agentId` is `a` +
16 lowercase hex; tool call ids `toolu_*`; API message ids `msg_*`; request
ids `req_*`. The schema annotates but does not assert `format`.

## 5. Quirks and gotchas

- **`message.content` union**: plain string on typed user prompts; block
  array for tool results / images / documents. Handle both.
- **`toolUseResult` tri-type** (string | object | array): string for plain or
  error text, else tool-specific structures (`stdout/stderr/interrupted`,
  `file{filePath,content,numLines,startLine,totalLines}`,
  `oldString/newString/structuredPatch`, `todos`, `content` block arrays,
  ...). Advisory duplicate of the `tool_result` block — richer, but not what
  the model saw. Open vocabulary; the schema deliberately does not enumerate
  its keys.
- **`tool_result.content` union**: string, or array of
  `text` / `image` / `tool_reference` blocks (`tool_reference` =
  `{type, tool_name}`, seen for deferred-tool loading).
- **Error assistant rows**: `error` (e.g. `authentication_failed`,
  `max_output_tokens`, `invalid_request`), `isApiErrorMessage: true`, and
  optionally `apiError`/`apiErrorStatus`/`errorDetails`; `requestId` may be
  absent. `message` is still present (27 instances).
- **Refusal fallback**: `system` subtype `model_refusal_fallback` with
  `direction: "retry"`, `trigger: "refusal"`, `originalModel`/`fallbackModel`,
  `apiRefusalCategory` (string|null), `refusedUserMessageUuid`; the paired
  assistant content block type `fallback` (`{from:{model}, to:{model}}`)
  marks the switch in-stream (5 blocks observed).
- **`user` rows that are not operator prompts**: `isMeta: true` rows
  (local-command caveats and similar harness injections, 152), peer/mesh
  messages with `origin.kind: "peer"` carrying `from`/`senderTaskId`/`body`,
  and `origin.kind` `task-notification`/`coordinator`/`human` provenance
  marks. `promptSource: "sdk"` marks SDK-driven prompts.
- **`queue-operation` privacy note**: `enqueue` rows duplicate the full
  prompt text in `content`; `dequeue`/`remove` carry no content. Redaction
  passes must cover them, not just `user` rows.
- **Numeric sloppiness**: `retryInMs` observed as both int and float; schema
  says `number`.
- **Null-only placeholders**: `message.stop_details` and `message.container`
  were only ever `null` (schema types them `null`; widen on first sighting).
  `apiRefusalExplanation` only null — schema allows string by symmetry with
  `apiRefusalCategory`, a documented judgment call, the schema's only
  deviation from strict observation.
- **`stop_reason: null`** occurs on 5,902 assistant lines (streams cut by
  interrupts or mid-response persistence), alongside
  `tool_use`/`end_turn`/`stop_sequence`/`max_tokens`.
- **Presence dips**: `entrypoint` absent on ~4% of message rows (notably one
  2.1.215-era session); `requestId` absent on error rows; exactly 1 of 1,250
  `last-prompt` rows lacks `lastPrompt`. Hence these are optional in the
  schema despite looking universal in small samples.
- **`system.toolUseID`** (note the casing) on `stop_hook_summary` rows is a
  UUID of the hook callback record — not a `toolu_*` id despite the name.
- **`gitBranch` can be `"HEAD"`** (detached), and `cwd` is wherever the
  session ran (scratchpads, worktrees) — do not assume it is a project root.
- **Live files grow**: the store is append-mostly and the current session's
  file (and bookkeeping rows like `last-prompt`) update while you read.

### Deviation from the meta-file doctrine

The interop plan's field doctrine listed sidecar meta
`toolUseId`/`parentAgentId`/`spawnDepth`/`agentType` as load-bearing. Observed
presence across 111 real meta files: `agentType` 111, `description` 106,
`toolUseId` 103, `spawnDepth` 100, `parentAgentId` 41. Requiring more than
`agentType` would reject most real files (`parentAgentId` is definitionally
absent at depth 1; workflow subagents lack `toolUseId`). The schema therefore
requires only `agentType` and the adapter must branch on which linkage fields
are present (§4.6).

## 6. Noise an importer may skip

Preserve every raw line as evidence (per the interop plan), but the following
need not become semantic canonical events:

- `queue-operation`, `last-prompt`, `ai-title`, `custom-title`, `mode`,
  `permission-mode`, `file-history-snapshot` — uuid-less UI/bookkeeping
  state, rewritten repeatedly (titles/last-prompt appear once per turn).
  Titles are worth harvesting once as session metadata.
- `pr-link`, `frame-link` — session metadata at most (external references).
- `attachment` — reconstructable context-injection bookkeeping (skill/agent/
  tool listings, reminders, file snippets). Large, noisy, model-visible only
  indirectly. `attachment.type` has 19 observed variants (schema lists them).
- `system` subtype `stop_hook_summary` (505 of 536 system rows) — hook
  telemetry (`hookCount`/`hookInfos`/`preventedContinuation`/...).
- Advisory fields on kept rows: `toolUseResult` (prefer the `tool_result`
  block as the model-visible truth, keep toolUseResult as raw evidence),
  duplicated `message.usage` across lines of one response,
  `classifierMetaLines`.
- Sidecar `tool-results/` files — overflow raw tool output; referenced
  implicitly (by `toolu_*` id / task id filename), useful as evidence blobs.
- `sessions-index.json` — derivable listing (`{version, entries[], originalPath}`,
  entry ≈ `{sessionId, fullPath, fileMtime, firstPrompt, summary,
  messageCount, created, modified, gitBranch, projectPath, isSidechain}`).

Conversation truth an importer must NOT skip: `user`/`assistant` rows
(including `isMeta`/`isCompactSummary` variants), `system` subtypes
`compact_boundary`, `api_error`, `model_refusal_fallback`, `local_command`,
and sidecar `started`/`result` rows.

## 7. Synthesized structural examples

Minimal user prompt (chain root):

```json
{"parentUuid":null,"isSidechain":false,"type":"user",
 "message":{"role":"user","content":"Count the lines in notes.txt"},
 "uuid":"00000000-0000-0000-0000-00000000000a","timestamp":"2026-07-25T12:00:00.000Z",
 "sessionId":"00000000-0000-0000-0000-000000000001","cwd":"/home/user/proj",
 "version":"2.1.118","gitBranch":"main","userType":"external",
 "entrypoint":"cli","promptId":"00000000-0000-0000-0000-00000000000b"}
```

Assistant tool call (one block per line; same `message.id` as its sibling
thinking/text lines):

```json
{"parentUuid":"00000000-0000-0000-0000-00000000000a","isSidechain":false,"type":"assistant",
 "message":{"model":"claude-haiku-4-5-20251001","id":"msg_000","type":"message","role":"assistant",
   "content":[{"type":"tool_use","id":"toolu_000","name":"Read",
               "input":{"file_path":"/home/user/proj/notes.txt"},"caller":{"type":"direct"}}],
   "stop_reason":"tool_use","stop_sequence":null,"stop_details":null,
   "usage":{"input_tokens":10,"output_tokens":50}},
 "requestId":"req_000","uuid":"00000000-0000-0000-0000-00000000000c",
 "timestamp":"2026-07-25T12:00:01.000Z","sessionId":"00000000-0000-0000-0000-000000000001",
 "cwd":"/home/user/proj","version":"2.1.118","gitBranch":"main","userType":"external"}
```

Tool result carrier:

```json
{"parentUuid":"00000000-0000-0000-0000-00000000000c","isSidechain":false,"type":"user",
 "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_000",
   "content":"1\talpha\n2\tbravo\n"}]},
 "toolUseResult":{"type":"text","file":{"filePath":"/home/user/proj/notes.txt",
   "content":"alpha\nbravo\n","numLines":2,"startLine":1,"totalLines":2}},
 "sourceToolAssistantUUID":"00000000-0000-0000-0000-00000000000c",
 "uuid":"00000000-0000-0000-0000-00000000000d","timestamp":"2026-07-25T12:00:02.000Z",
 "sessionId":"00000000-0000-0000-0000-000000000001","cwd":"/home/user/proj",
 "version":"2.1.118","gitBranch":"main","userType":"external"}
```

Sidecar meta file (`subagents/agent-a1b2c3d4e5f607182.meta.json`):

```json
{"agentType":"general-purpose","description":"Survey the fixture corpus",
 "toolUseId":"toolu_001","spawnDepth":1}
```

## 8. Validation record (2026-07-25)

Validator: python `jsonschema` 4.26.0, Draft 2020-12; per-line dispatch on the
`type` const with a 4,000-line cross-check against the root `oneOf` (identical
results; the consts are disjoint).

| set | files | lines/objects | valid | violations |
|---|---|---|---|---|
| fresh captures (2.1.118) | 3 | 40 | 40 | 0 |
| full corpus (main + sidecars) | 142 | 36,279 | 36,279 | 0 |
| agent meta files | 111 | 111 | 111 | 0 |

Zero unexplained violations; nothing was carved out as out-of-schema noise —
every observed line validates. A 17-case negative-control suite confirms the
schema rejects malformed records (missing required fields, wrong-typed
`isSidechain`, tool blocks without ids, unknown `type` values) while accepting
unknown *fields* (`additionalProperties: true` everywhere, per doctrine).

Caveats: single-machine corpus, one operator's usage patterns; `started`/
`result` (n=5/4), `fallback` blocks (n=5), `document` blocks (n=4) and
`permission-mode` (n=1) are thin-sample shapes; re-derive against a second
machine before treating their `required` sets as settled.
