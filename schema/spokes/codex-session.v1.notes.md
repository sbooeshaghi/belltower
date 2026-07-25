# Codex CLI rollout format — derivation notes (v1)

Companion to [`codex-session.v1.schema.json`](./codex-session.v1.schema.json).
The schema is the defined truth the Belltower Codex adapter is built against
([interop plan](../../docs/planning/harness-interop-plan-20260725.md), Phase 1
ingress / Phase 2 egress). This file records where that truth came from, the
invariants an importer may lean on, and the places the format will bite.

All examples below are synthesized. No real transcript text, path, or
identifier from the surveyed corpus appears here or in the schema.

## 1. Provenance

Derived post-hoc by surveying the complete Codex store on one operator
machine, then validating the derived schema back against it.

| | |
|---|---|
| Store surveyed | `~/.codex/sessions/YYYY/MM/DD/rollout-<ISO8601>-<uuid>.jsonl`, `~/.codex/archived_sessions/*.jsonl`, `~/.codex/session_index.jsonl` |
| Rollout files | 969 (807 live + 162 archived); 967 non-empty, 2 zero-byte |
| Lines | 1,510,157 JSON objects; 0 unparseable, 0 non-object |
| Date span | 2025-10-14 → 2026-07-25 |
| Files by year | 2025: 1 · 2026: 968 (live 2026-02:23, 03:21, 04:159, 05:96, 06:85, 07:422; archived 2026-02:2, 04:15, 05:28, 07:117) |
| `cli_version` values | 46 distinct, `0.46.0` → `0.146.0-alpha.3` (recorded in every `session_meta`, so version is always recoverable per session) |
| Originators | `Codex Desktop` (4896 metas), `codex-tui` (106), `codex_cli_rs` (39), `xtend` (4), `codex_exec` (2) |
| Index file | `~/.codex/session_index.jsonl`, 700 records, 681 distinct ids |
| Fresh captures | 3 sessions, `codex-cli 0.137.0`, 2026-07-25, `codex exec --sandbox read-only --skip-git-repo-check` |

The corpus is heavily skewed toward one operator's multi-agent usage: 854 of
967 non-empty files are spawned sub-agent threads. That is good for
sub-agent coverage and bad for coverage of plain single-session TUI use, so
the rarer TUI/`codex exec` paths were covered by the fresh captures instead.

### Fresh captures

Auth was live (`~/.codex/auth.json`, ChatGPT-subscription mode). Three runs:

1. `"Reply with exactly OK"` on the configured default model — rejected by the
   API with HTTP 400 (`the '<model>' model requires a newer version of
   Codex`). **The rollout file was still created and contains no trace of the
   error**: `task_started` → synthetic messages → the user message →
   `task_complete` with `last_agent_message: null`. Provider errors are a
   stdout-only concern; an importer sees a turn that simply produced nothing.
2. Same prompt pinned to a working model — 12 lines, one clean turn.
3. `"Run the shell command pwd and tell me its output verbatim."` — 14 lines,
   exercising `function_call` (`exec_command`) → `function_call_output`.

Capture 3's whole-file shape, which is the minimal skeleton of every Codex
session:

```
0  session_meta                     id/cwd/originator/cli_version/source/base_instructions
1  event_msg/task_started           turn_id
2  response_item/message            role=developer, 4 synthetic instruction blocks
3  response_item/message            role=user, 2 synthetic blocks (AGENTS.md, environment_context)
4  turn_context                     turn_id, cwd, model, sandbox_policy, ...
5  response_item/message            role=user   <- the REAL prompt
6  event_msg/user_message           mirrors line 5
7  response_item/function_call      name=exec_command, call_id=call_XXXX
8  response_item/function_call_output  call_id=call_XXXX
9  event_msg/token_count
10 event_msg/agent_message          phase=final_answer
11 response_item/message            role=assistant, phase=final_answer
12 event_msg/token_count
13 event_msg/task_complete          turn_id, last_agent_message
```

## 2. Record inventory

Seven top-level `type` values, ever. Every line in the corpus is exactly
`{"timestamp", "type", "payload"}` — no line carried a fourth key, and every
timestamp is `YYYY-MM-DDTHH:MM:SS.mmmZ`.

| line type | count | role |
|---|---:|---|
| `response_item` | 773,449 | model-conversation items — **authoritative content** |
| `event_msg` | 701,177 | UI/telemetry event stream |
| `turn_context` | 25,472 | per-turn execution settings |
| `session_meta` | 5,050 | session header (recurs; see §3) |
| `compacted` | 2,470 | history compaction record |
| `world_state` | 2,126 | client ambient-context snapshot |
| `inter_agent_communication_metadata` | 413 | marker beside an agent→agent message |

`response_item` payload types (10 observed):

| payload type | count | linkage key |
|---|---:|---|
| `function_call` | 209,662 | `call_id` |
| `function_call_output` | 209,620 | `call_id` |
| `reasoning` | 148,195 | — |
| `message` | 107,526 | — |
| `custom_tool_call_output` | 47,722 | `call_id` |
| `custom_tool_call` | 47,715 | `call_id` |
| `web_search_call` | 2,366 | `id` (often absent) |
| `agent_message` | 413 | `author`/`recipient` agent paths |
| `tool_search_call` | 115 | `call_id` |
| `tool_search_output` | 115 | `call_id` |

An eleventh payload type, `compaction`, occurs **only** nested inside a
`compacted` line's `replacement_history`, never as a standalone line.

`event_msg` payload types (24 observed): `token_count` (366,554),
`agent_message` (126,347), `agent_reasoning` (96,325), `patch_apply_end`
(29,399), `user_message` (18,507), `task_started` (17,311), `task_complete`
(16,926), `exec_command_end` (12,236), `web_search_end` (4,508),
`mcp_tool_call_end` (3,975), `context_compacted` (3,118),
`thread_settings_applied` (2,628), `sub_agent_activity` (2,189),
`turn_aborted` (269), `collab_agent_spawn_end` (165), `thread_name_updated`
(137), `collab_waiting_end` (130), `view_image_tool_call` (125),
`collab_close_end` (88), `item_completed` (84), `thread_goal_updated` (79),
`thread_rolled_back` (58), `collab_agent_interaction_end` (18),
`collab_resume_end` (1).

### Load-bearing field set

Fields the schema marks `required` — the ones an importer cannot proceed
without, and the only place strictness lives:

- envelope: `type`, `timestamp`, `payload`
- `session_meta.payload`: `id`, `cwd`
- `response_item.payload`: `type`
  - `message`: `role`, `content` (array; blocks require `type`, and `text`
    for `input_text`/`output_text`, `image_url` for `input_image`)
  - `function_call`: `name`, `arguments`, `call_id`
  - `function_call_output`: `call_id`, `output`
  - `custom_tool_call`: `call_id`, `name`, `input`
  - `custom_tool_call_output`: `call_id`, `output`
  - `reasoning`: `summary`, `encrypted_content`
  - `tool_search_call` / `tool_search_output`: `call_id`
  - `agent_message`: `content`
  - `web_search_call`: nothing beyond `type` — see §3.3
- `event_msg.payload`: `type`; plus `message` on `user_message`/`agent_message`,
  `text` on `agent_reasoning`, `turn_id` on the turn-lifecycle events,
  `call_id` on the tool-end events, `agent_thread_id`+`event_id` on
  `sub_agent_activity`
- `turn_context.payload`: `cwd` (**not** `turn_id` — absent on 0.46.0)
- `compacted.payload`: `replacement_history`
- `sessionIndexRecord`: `id`

Everything else observed is an optional typed property, and every object is
open. One structural asymmetry is deliberate: the *root* `oneOf` over the
seven line types is closed, so a genuinely new record kind surfaces as a
validation failure the adapter owner must look at; `response_item`/`event_msg`
payload variants use `if`/`then`, so a new payload type passes with only
`type` required. That matches observed churn — the top-level set grew by two
in nine months while the payload set grew from 10 to 34.

## 3. Linkage invariants

These are what the importer builds on. Each was checked across the full
corpus; counts are exact.

### 3.1 File and session identity

- **`session_meta` is always line 0.** 964/964 non-empty files at survey time
  (967/967 including the fresh captures).
- **The filename UUID equals the first `session_meta.payload.id`.** 802/802
  live rollout files checked, zero mismatches. Filename is
  `rollout-<local ISO8601 with dashes>-<id>.jsonl`; the embedded time is
  **local**, while all in-file timestamps are UTC. Do not derive session start
  from the filename.
- **`session_meta` recurs.** 94 files carry more than one; one file carries
  556. Two distinct causes:
  - *Desktop resume*: the same `id` re-emitted on each reopen (append-only file).
  - *Fork*: 85 files contain ≥2 distinct ids. In **all 85**, line 0 is the
    file's own meta carrying `forked_from_id`, and line 1 is the **source
    thread's** `session_meta` whose `id` equals that `forked_from_id`.
  An importer must key off line 0 and treat later metas as history/resume
  markers, not as new sessions.
- **Ordering is file order.** Timestamps are not a safe sort key: 4 of 969
  files contain at least one backwards step.

### 3.2 Tool call ↔ output pairing

**The linkage key is `call_id`, never `id`.** `payload.id` is the provider's
item id and its prefix encodes the item kind (`msg_`, `rs_`, `fc_`, `fco_`,
`ctc_`, `ctco_`, `ws_`, `tsc_`, `tso_`, `amsg_`); it is present on only
0.8–77% of items depending on type and version, and is useless for pairing.
`call_id` (prefix `call_`) is present on 100% of `function_call`,
`function_call_output`, `custom_tool_call`, `custom_tool_call_output`,
`tool_search_call`, `tool_search_output`.

Checked over 257,456 output records:

- **No output ever preceded its call** in the same file. Zero exceptions.
- 45 calls had no output at all — dangling, and the honest-terminalization
  case for ingress. By tool: `spawn_agent` 30, `exec_command` 7, `close_agent`
  3, `exec` (custom) 3, `write_stdin` 2. They cluster at turn ends and
  interrupts.
- 10 outputs referenced a `call_id` that had already been answered
  (duplicate/retried output). Last-write-wins is the only defensible rule.

`function_call.arguments` is a **JSON-encoded string** on 100% of 209,662
observations, never a nested object. `tool_search_call.arguments` is the
exception: there it is a real object.

### 3.3 The `web_search_call` exception

`web_search_call` carries no `call_id` at all, and 1,906 of 2,366 carry no
`id` either. The paired `event_msg/web_search_end` puts a `ws_…` value in its
own `call_id` field, which references the search item's **`id`**. When the id
is absent the pairing is unrecoverable: 4,048 of 4,508 `web_search_end`
events dangle by construction. Treat search calls as unpaired evidence and
carry the `web_search_end` results as their own record.

### 3.4 Cross-stream (`event_msg` → `response_item`) references

`event_msg` records referencing a `call_id` from the response-item stream:

| event | resolves in-file | does not |
|---|---:|---:|
| `exec_command_end` | 12,236 | 0 |
| `view_image_tool_call` | 125 | 0 |
| `collab_*_end` | 402 | 0 |
| `patch_apply_end` | 11,892 | 17,507 |
| `mcp_tool_call_end` | 654 | 3,321 |
| `web_search_end` | 460 | 4,048 (see §3.3) |

`patch_apply_end` and `mcp_tool_call_end` frequently dangle because the
originating call was replaced by compaction while the event stream kept
everything. For those two, the event is often **the only surviving record**
of an edit or an MCP result — so "event_msg is skippable" is not universally
true (§4).

### 3.5 Sub-agent / child-thread linkage

Three generations of the same idea; an adapter needs all three.

- **`source.subagent.thread_spawn.parent_thread_id`** (711 metas) — most
  reliable child→parent edge. `agent_path`/`agent_role` inside it are often
  null (675/711 and 82/711). A second shape, `source.subagent.other:
  "<string>"` (144 metas), marks a child with no spawn descriptor.
- **`parent_thread_id`** at the top of `session_meta` (591 metas) — same edge,
  different location, newer versions.
- **Parent-side records**: `event_msg/collab_agent_spawn_end.new_thread_id`
  (2026-04 vintage) and `event_msg/sub_agent_activity.agent_thread_id`
  (2026-07 vintage). **A child thread is always its own file**, never inlined
  in the parent (unlike Claude Code sidecars): all 176 distinct spawn targets,
  and all 2,189 `sub_agent_activity.agent_thread_id` values, had a
  materialized rollout file.
- **Spawn-call → child-session edge** (what `parent_external_ref` wants):
  `collab_agent_spawn_end.call_id` resolved to a `spawn_agent` `function_call`
  in the same file on 165/165. Its successor is weaker —
  `sub_agent_activity.event_id` resolved to an in-file `function_call` only
  369 of 2,189 times (`spawn_agent` 30 for `kind: "started"`, `send_message`
  290 and `followup_task` 49 for `kind: "interacted"`); the other 83% dangle,
  so the edge must fall back to `agent_thread_id` + the child's own
  `parent_thread_id`.
- `agent_path` (`/root`, `/root/<task>`, depth ≥2 observed) gives the tree
  position independently of ids.

### 3.6 `session_index.jsonl`

`{"id", "thread_name", "updated_at"}` and nothing else, on all 700 records.
It is a **title cache for the picker UI, not a session inventory**:

- 700 records for 681 distinct ids (append-only; later record wins).
- 525 ids had a live rollout file; 156 did not (archived or deleted).
- 279 rollout files had no index entry — including all three fresh
  `codex exec` captures. `codex exec` never touches the index.
- `updated_at` has 6 fractional digits, unlike rollout timestamps' 3.

Use it only to recover a human-readable session title (also available from
`event_msg/thread_name_updated`). Never use it to enumerate sessions —
walk the `sessions/` tree.

### 3.7 Which stream is authoritative

**`response_item` is authoritative for content.** It is what Codex replays to
the model on resume; it is the conversation. `event_msg` is the transcript-UI
projection and does **not** pair 1:1 with it (126,347 `agent_message` events
vs 72,907 assistant messages — a single response is emitted as several
phase-tagged events).

Three exceptions where `event_msg` is the only source of truth, plus a
conditional fourth below:

1. **`event_msg/user_message` is the best available discriminator between a
   real operator turn and a synthetic one.** Codex persists injected context
   as ordinary `message`/`role=user` response items, so nothing in the
   response-item stream distinguishes them; only operator input normally also
   emits a `user_message` event. It is not exact. Matching every user-role
   message's block text against the same file's `user_message` events
   (29,023 items):

   | | mirrored by an event | not mirrored |
   |---|---:|---:|
   | plain first block | 15,370 | 2,590 |
   | tag-prefixed first block | 696 | 10,367 |

   i.e. 96% precision, 86% recall on its own. The misses are concentrated in
   resumed and forked files, where replayed response items arrive without
   their original events. **Use the event mirror AND the tag-prefix heuristic
   (§4) together**, and treat disagreement as "synthetic" — over-attributing
   machine text to the operator is the worse failure.
2. **`token_count`** — the sole record of usage/cost, in no response item.
3. **`exec_command_end` / `patch_apply_end` / `mcp_tool_call_end` /
   `web_search_end`** — exit codes, per-file diffs, MCP payloads, and search
   results that the response-item `output` string does not carry, and that
   survive compaction when the calls themselves do not.

`agent_reasoning` events are also the only *plaintext* trace of reasoning
whose `response_item` counterpart has an empty `summary` (§5.2).

## 4. Skippable bookkeeping / UI noise

Safe to drop from canonical events on ingress (still preserved as raw lines
per the plan's evidence rule):

| record | count | why |
|---|---:|---|
| `event_msg/agent_message` | 126,347 | duplicate of assistant `message`; keep the response item |
| `event_msg/agent_reasoning` | 96,325 | duplicate of `reasoning.summary` — **except** when that summary is empty |
| `event_msg/context_compacted` | 3,118 | bare `{type}`; the `compacted` line carries the content |
| `world_state` | 2,126 | client ambient-context snapshot; no conversation content |
| `event_msg/thread_settings_applied` | 2,628 | settings echo; `turn_context` is the durable version |
| `event_msg/thread_name_updated` | 137 | title only |
| `event_msg/thread_goal_updated` | 79 | Desktop goal-tracker widget state |
| `event_msg/item_completed` | 84 | UI item render (only `Plan` items observed) |
| `inter_agent_communication_metadata` | 413 | one-bit `{trigger_turn}` marker beside the real `agent_message` item |
| `turn_context` | 25,472 | not noise, but *settings*, not content — map to session/turn attributes, not to events |

Do **not** skip `event_msg/user_message` (§3.7), `token_count`, the four
tool-end events, `task_started`/`task_complete`/`turn_aborted`,
`sub_agent_activity`/`collab_*`, or `thread_rolled_back`.

Synthetic (non-human) `message` records that must not be rendered as operator
or agent speech — identified by the first content block's opening tag:

- `role=user`: `<environment_context>` (4,746), `<subagent_notification>`
  (4,528), `# AGENTS.md …` project-instruction injection (146),
  `<in-app-browser-context>` (676), `<codex_internal_context>` (359),
  `<recommended_plugins>` (318), `<turn_aborted>` (250),
  `<task-notification>` (34), `<user_shell_command>` (4)
- `role=developer`: `<permissions instructions>` (4,529),
  `<apps_instructions>`, `<skills_instructions>`, `<plugins_instructions>`,
  `<collaboration_mode>` (198), `<model_switch>` (89), `<multi_agent_mode>`
  (63), `<app-context>` (41), `<personality_spec>` (3) — all of `developer`
  is synthetic
- `role=assistant`: `<proposed_plan>` (80) is a structured plan, not prose

Only 17,957 of 29,088 user-role messages surveyed had plain text as their
first block, and not all of those are human either — cross-check with
`event_msg/user_message`.

## 5. Quirks and format drift

### 5.1 2025 → 2026 drift (concrete)

The single 0.46.0 session (2025-10) differs from current files in ways an
importer must tolerate:

| aspect | 0.46.0 (2025-10) | 2026 |
|---|---|---|
| system prompt | `session_meta.instructions: null` | `session_meta.base_instructions: {text}` |
| `session_meta` keys | 7 (`id, timestamp, cwd, originator, cli_version, instructions, source`) | up to 19 on one record, 22 distinct corpus-wide (`git`, `session_id`, `thread_source`, `memory_mode`, `history_mode`, `dynamic_tools`, `context_window`, `multi_agent_version`, `parent_thread_id`, `agent_*`, …) |
| `turn_context.turn_id` | **absent** | present on 100% |
| `turn_context.sandbox_policy` | discriminator key `mode` | discriminator key `type` |
| turn lifecycle | no `task_started`/`task_complete` at all | 34,237 such events |
| shell tool | `shell` (132 calls) | `exec_command` + `write_stdin`/`wait` |
| `rate_limits` | `{primary, secondary}` with `resets_in_seconds` | `{credits, limit_id, limit_name, plan_type, primary, secondary, individual_limit?, rate_limit_reached_type?, spend_control_reached?}` with `resets_at` |
| `user_message.kind` | `"plain"` | field dropped |
| `message.phase` | absent | `commentary` / `final_answer` from 2026-02 |
| top-level line types | 4 | 7 (`compacted` + `world_state` from 2026-02, `inter_agent_communication_metadata` from 2026-07) |

Within 2026 the format keeps moving:

- **Turn attribution renamed.** `payload.metadata.turn_id` (2026-02 → 2026-04)
  → `payload.internal_chat_message_metadata_passthrough.turn_id` (2026-05 →).
  Both shapes still appear; both are `{turn_id}` only.
- **`event_msg/exec_command_end` exists only in 2026-04** (12,236 of 12,236
  lines). Before and after, shell results are carried solely by
  `function_call_output` / `custom_tool_call_output` strings. Any adapter that
  depends on exit codes must degrade gracefully.
- **The multi-agent event vocabulary was replaced wholesale.** `collab_*`
  events are 2026-04 only; `sub_agent_activity`, `response_item/agent_message`
  and `inter_agent_communication_metadata` are 2026-07 only. Nothing bridges
  them.
- **`message.phase`** is not a clean version cut: present from 2026-02 onward,
  but still absent on 20–35% of messages in every later month (user-role and
  synthetic messages never carry it). Never key logic on its presence.
- **The `exec` custom tool is feature-flagged, not versioned.** `apply_patch`
  is present in every month; `exec` appears in 2026-02, -04, -06, -07 and is
  entirely absent in 2026-03 and -05, while `exec_command` function calls run
  continuously throughout. An adapter must handle shell work arriving as
  `function_call`, as `custom_tool_call`, or as both in one session.
- **`user_instructions` moved.** `turn_context.user_instructions` (project
  AGENTS.md inlined) is used in 2026-02 (6,331), 2026-03 (862) and 2026-04
  (47), then vanishes. The `# AGENTS.md …` synthetic user message exists in
  parallel from 2026-02 and is the only carrier from 2026-06 on.
- **`compacted.window_id` is `int` in older files and a UUID `string` in newer
  ones** — a genuine type union, not a survey artifact.
- **`tool_search_call`/`tool_search_output`** (deferred tool loading) appear
  from 2026-04 and are client-executed (`execution: "client"`).

### 5.2 Encrypted reasoning

`reasoning` items always carry a non-null `encrypted_content` string
(148,195/148,195) and always an array `summary` — but **the summary is empty
on 131,928 of them (89%)**. `content` is present-and-null on 71% and was never
non-null. So for ~9 out of 10 reasoning items the rollout contains no
human-readable reasoning at all; the blob is opaque, account-scoped, and
cannot be decrypted by Belltower. Treat it as evidence to preserve verbatim,
never as text to render. Where `event_msg/agent_reasoning` exists it is the
only readable trace — and it is absent for entire months (none at all in
2026-03 or 2026-05, depending on the reasoning-summary setting).

### 5.3 Compaction

A `compacted` line carries `replacement_history`: an array of **bare
response-item payloads** (no envelope) that becomes the model's history from
that point. The pre-compaction items remain earlier in the same file.

- 2,470 compaction records containing 411,656 replacement items — 53% as many
  items as the corpus's entire `response_item` stream, and 3.8× the number of
  real `message` items. **An importer that concatenates response items and
  also expands `replacement_history` will double-count massively.** Treat
  `compacted` as a marker; do not expand.
- `replacement_history` item types: `message` (409,186) and `compaction`
  (2,470) — the latter being an encrypted summary-of-summaries item that never
  appears standalone.
- `compacted` is always followed by `event_msg/context_compacted` (3,118 vs
  2,470 — the event also fires in cases with no `compacted` line).

### 5.4 Turn aborts, rollbacks, and invisible errors

- `turn_aborted` (269, `reason: "interrupted"`): any tool call still open at
  that point is terminated without result.
- `thread_rolled_back` (58) with `num_turns`: **the rolled-back lines stay in
  the file.** A naive file-order replay resurrects abandoned turns. There is
  no marker on the abandoned lines themselves.
- Provider/API errors are **never written to the rollout** (proven by fresh
  capture 1). The only trace is `task_complete` with `last_agent_message:
  null` and no assistant `message` for that `turn_id`.
- 2 rollout files are zero-byte. A reader must tolerate an empty session file.

### 5.5 Miscellany

- `codex exec --ephemeral` runs "without persisting session files to disk"
  (CLI help, 0.137.0; not exercised here). Such sessions leave nothing to
  adopt, by design.
- `event_msg/user_message.local_images` are absolute paths to temp files
  outside the rollout, and are routinely gone by import time; `images` holds
  inline `data:` URIs for the same content and is the durable copy.
- `patch_apply_end.changes` is keyed by **absolute host path**, so egress
  retargeting to a new cwd must rewrite those keys or accept stale ones.
- Tool output is a `string` in 232,885 records and an array of content blocks
  in 24,457. Both encodings appear in the same version and even the same
  session; handle the union.
- Archived sessions (`~/.codex/archived_sessions/`) use the identical line
  format and the same filenames; they are simply moved, not transformed.

## 6. Validation

Validator: `jsonschema` 4.26.0, Draft 2020-12, Python 3.13.
`Draft202012Validator.check_schema` passes.

| pass | records | violations |
|---|---:|---:|
| fresh captures, every line of all 3 files | 34 | 0 |
| month-spread sample: 29 files × up to 120 random lines, ≥1 file from every month present (2025-10 through 2026-07 plus archived) | 2,883 | 0 |
| rare-type sweep: every line of the 23 low-frequency discriminators, capped at 400 each | 5,384 | 0 |
| `session_index.jsonl`, all records | 700 | 0 |
| **whole corpus, every line of all 969 files** | **1,510,157** | **0** |

All 39 observed discriminators are exercised (5 bare line types + 10
`response_item` payload variants + 24 `event_msg` payload variants).
One iteration was needed: the first draft failed on 9 sampled
`session_meta` lines because `source.subagent.thread_spawn.agent_path` and
`.agent_role` are nullable (675 and 82 of 711 occurrences); both were widened
to `["string","null"]`. No other change was required, and there are **no
deliberate exclusions** — nothing in the corpus is knowingly left failing.

Reproduce with the scratch scripts used here (`scan_corpus.py`, `deep2–4.py`,
`validate.py`, `validate_full.py`); they emit shapes and counts only, never
transcript text.

## 7. Consequences for the adapter

- Ingress reads `response_item` in file order for content, and mines
  `event_msg` for the handful of things that live nowhere else (§3.7). Every
  line is preserved as the raw chunk of whatever it produced, including the
  noise in §4.
- The real-vs-synthetic user split is the single highest-risk mapping
  decision, and **no single field decides it** (§3.7 exception 1 + §4). Get it
  wrong and adopted sessions attribute machine-generated context to the
  operator. This deserves a fixture-backed test on both a fresh session and a
  resumed/forked one.
- Child sessions are separate files and always materialized — a clean fit for
  `parent_external_ref` and tree bundles. Prefer the child's own
  `source.subagent.thread_spawn.parent_thread_id` / `parent_thread_id` as the
  edge; the parent-side spawn-`call_id` is exact only for the 2026-04
  `collab_*` generation (§3.5). Forked sessions (`forked_from_id`) are a
  second, distinct DAG edge.
- Egress must write `function_call`/`function_call_output` with fresh
  `call_id`s (`call_` prefix), `message` items with `input_text`/`output_text`
  blocks, and a `session_meta` line 0 whose `id` matches the filename UUID
  written with a **local**-time prefix. `reasoning` items cannot be
  synthesized honestly — there is no way to produce a valid
  `encrypted_content` — so Belltower thinking must egress as assistant text or
  a bracketed note, never as a forged reasoning item.
- Whether a `session_index.jsonl` entry is needed for a handed-off session to
  appear in the Desktop/TUI picker was **not** tested here (it is a
  side-effectful write into the operator's live store). What is measured: 279
  rollout files, including every `codex exec` run, have no index entry and are
  still present as rollout files. Verify the picker behaviour before deciding
  whether egress writes an index line.
