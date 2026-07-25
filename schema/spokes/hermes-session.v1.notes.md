# Hermes Agent session format — derivation notes (v1)

Companion to [`hermes-session.v1.schema.json`](./hermes-session.v1.schema.json).
The schema describes on-disk observed reality; this file records how it was
derived, the invariants an importer may rely on, and known drift in newer
hermes revisions. No transcript content is reproduced here; all example
values are synthetic.

## Provenance

- Corpus: 12 session documents in `~/.hermes/sessions/` — 10 pre-existing
  (2026-03-23 through 2026-05-16, 1–16 messages each) plus 2 fresh captures
  made for this derivation on 2026-07-25 via
  `hermes chat -Q -q "<tiny prompt>"` (one plain reply, one single
  `terminal` tool round trip; both succeeded against a local
  OpenAI-compatible backend). One `request_dump_*.json` file also present
  (different format, documented below, excluded from the schema).
- Writer version: installed Hermes Agent v0.6.0 (2026.3.30) at
  `~/.local/bin/hermes`. Backends observed across the corpus: the ChatGPT
  Codex backend (`https://chatgpt.com/backend-api/codex`, model `gpt-5.4`),
  LM Studio (`http://127.0.0.1:1234[/v1]`), Ollama
  (`http://127.0.0.1:11434/v1`), models `qwen3.5-9b-mlx` / `qwen3.5:latest`.
- Source read (read-only) at `~/.hermes/hermes-agent`, checked out at
  `v2026.3.30-125-ge905768f` — 125 commits ahead of the installed tag, so
  both the tag (`git show v2026.3.30:run_agent.py`) and HEAD were compared:
  - `run_agent.py` — `_save_session_log` (~L2174, the single writer),
    `_persist_session` (~L1712), `_build_assistant_message` (~L5019),
    `_dump_request_debug` (~L2088), `_split_responses_tool_id` (~L2940),
    `_deterministic_call_id` (~L2925), `_flush_messages_to_session_db`
    (~L1725).
  - `utils.py` L12 — `atomic_json_write` (temp file + fsync + `os.replace`).
  - `hermes_state.py` — `get_messages_as_conversation` (~L952), the
    SQLite→messages reconstruction used by resume.
- Validation: `12/12` documents pass the schema
  (jsonschema 4.26.0, Draft 2020-12); zero linkage violations; zero
  `message_count` mismatches. Ten mutation tests confirm every required
  field rejects and the two deliberate tolerances (unknown roles, unknown
  extra fields) accept. The `request_dump` file fails as expected
  (deliberate exclusion — it is not a session document).

## Document structure

One session = one JSON document (not JSONL). Every observed file has
exactly the same ten top-level keys in one canonical order:

`session_id, model, base_url, platform, session_start, last_updated,
system_prompt, tools, message_count, messages`

`messages[]` is OpenAI chat-completions shaped with roles
`user`/`assistant`/`tool` only (corpus: 19 user, 27 assistant, 18 tool
across 12 files; first message is always `user`). `tools[]` is the full
OpenAI function-spec array of enabled tools (~29 entries, tens of KB),
re-embedded on every save.

## Persistence model — and what it means for import

`_save_session_log` rewrites the **entire document atomically after every
turn** (temp file + fsync + `os.replace`), with a monotonic no-shrink
guard: if the existing file's `message_count` exceeds the new message
list's length, the write is skipped (protects the JSON log from a resume
that loaded partial history from SQLite).

Contrast with the JSONL harnesses (Claude Code, Codex): there is no
per-line identity and no append boundary.

- A reader always sees a complete, self-consistent snapshot (never a
  torn write).
- Raw-evidence refs must cite `(session_id, message index)`, not byte
  offsets or line numbers.
- Re-import after the session grew cannot tail the file; it must re-read
  the whole document and diff by message-index prefix. The prefix is
  *typically* append-only but *not guaranteed*: source contains a
  rollback path that drops a malformed final assistant turn
  (`_get_messages_up_to_last_assistant`) and a pre-call sanitizer that can
  *insert* stub tool-result messages mid-history. Safest posture: treat
  each read as a fresh document version, not an append stream.
- `last_updated` + `message_count` change on every rewrite and are cheap
  staleness signals; recompute `message_count` on import rather than
  trusting it (it matched `len(messages)` in 12/12 files, but it is
  writer-maintained state, and the no-shrink guard reads it from disk).

**Resume truth lives elsewhere.** `hermes --resume` restores history from
SQLite (`~/.hermes/state.db`), *not* from this JSON (`cli.py` resume
docstring; `_preload_resumed_session`). The JSON log is a write-only
durable artifact from hermes's own perspective. Consequences:

- **Ingress**: the JSON is the right read source — it is the cleaned,
  complete log the agent saw.
- **Egress** (making a session hermes-resumable) would require writing
  SQLite rows too; producing only this JSON file is not sufficient.
  (Out of scope for the current plan; recorded so nobody assumes
  file-drop egress works.)
- **Lineage is invisible in the JSON.** Context compression ends a session
  and starts a new `session_id`; the parent link
  (`parent_session_id`) is recorded only in the SQLite `sessions` table.
  Cross-file ancestry cannot be recovered from the documents alone.

## Load-bearing fields and linkage invariants

Required by the schema (everything else optional):
`session_id`, `session_start`, `model`, `messages[]`; per message `role`;
`content` on assistant and tool messages; `tool_calls[].id` +
`function.name` + `function.arguments`; `tool_call_id` on tool messages.

Invariants verified across all 12 documents (34 tool calls):

1. **Tool linkage**: every `tool` message's `tool_call_id` equals the `id`
   of a `tool_call` in a *preceding* assistant message; 1:1, no orphans,
   no dangling calls, results appear in call order immediately after the
   calling turn. Multi-call turns observed with 2 calls (results follow
   in the same order); nothing bounds N.
2. **Order**: array order is the only ordering. Individual messages carry
   **no timestamps** — an importer must synthesize event times, bounded by
   `session_start`/`last_updated`. (This is the biggest fidelity gap
   versus the JSONL harnesses.)
3. **`id == call_id`** on every observed tool call; `response_item_id` is
   always present and `fc_`-prefixed — real Codex Responses item ids on
   the Codex backend, *synthesized* (`call_x` → `fc_x`) elsewhere.
4. **Identity**: the embedded `session_id` is authoritative, not the
   filename. They matched in 11/12 files; the mismatch
   (filename id ~8s older than embedded id) coincided with an aborted
   first turn (single `user` message, no assistant reply). Filename
   convention: `session_<session_id>.json`; id convention
   `YYYYMMDD_HHMMSS_<hex6>`, but the constructor accepts caller-supplied
   ids, so neither is enforced by the schema.
5. **Timestamps** are naive local time with microseconds and no offset
   (`2026-01-01T12:00:00.123456`) — not RFC 3339; do not parse with a
   strict `date-time` parser and do not assume UTC.
6. `finish_reason` observed: `stop`, `tool_calls`. Source can also write
   `incomplete` (Codex turns that produced only encrypted reasoning).
   Assistant messages on fallback paths in source can carry only
   `{role, content}` — hence `reasoning`/`finish_reason` optional.

## The request_dump file (excluded, importer-irrelevant)

`request_dump_<session_id>_<YYYYMMDD_HHMMSS_μs>.json`, written by
`_dump_request_debug` on retry exhaustion / provider 4xx debugging. Shape:

```json
{
  "timestamp": "<naive-iso>",
  "session_id": "<id of the session being attempted>",
  "reason": "max_retries_exhausted",
  "request": {
    "method": "POST",
    "url": "<base_url>/chat/completions | <base_url>/responses",
    "headers": { "Authorization": "Bearer <masked>", "Content-Type": "application/json" },
    "body": { "model": "...", "messages": [...], "tools": [...] }
  },
  "error": { "type": "APIConnectionError", "message": "..." }
}
```

`error` may additionally carry `status_code`, `request_id`, `code`,
`param`, `body`, `response_status`, `response_text`. The API key is
masked before writing. Skip these files on import (or attach as
diagnostic sidecar evidence); they duplicate the session's messages in
outbound-request form and are not session state.

## Quirks

- **Encrypted Codex reasoning**: `codex_reasoning_items[]` are opaque
  Responses-API items (`type: "reasoning"`, `rs_`-prefixed `id`,
  `encrypted_content` ciphertext, `summary[]` which may be empty).
  Hermes replays them verbatim to maintain reasoning continuity. Only
  `summary[].text` is renderable; preserve the rest byte-exact as raw
  evidence.
- **`base_url` is the provenance of the whole document, at last save.**
  Hermes rides the ChatGPT Codex backend via the Responses API
  (`api_mode: codex_responses`) yet still logs in chat-completions shape;
  local OpenAI-compatible servers use the same shape natively. A
  mid-session `/model` switch would retroactively relabel history —
  `model`/`base_url` mean "as of `last_updated`", not per-turn.
- **`system_prompt` snapshot per session**: the full rendered persona +
  instructions (tens of KB), duplicated in every document; `""` default.
  Good provenance, cheap to dedupe on import.
- **Assistant `content`** is never null (writer coalesces to `""`; empty
  on pure tool-call turns). It may embed literal `<think>...</think>`
  blocks: the writer converts a `REASONING_SCRATCHPAD` convention to
  `<think>` tags at save time, and `reasoning` may duplicate that text.
  `reasoning` is the observed `string|null` union; `reasoning_content`
  is a rare provider-style duplicate (1 of 27 assistant messages).
- **Tool result `content`** is a string, commonly stringified JSON
  (`{"output": ...}`). Hermes may inject a `_budget_warning` key into
  that JSON (or append text), truncate beyond 100k chars with a
  `[Truncated: ...]` marker, or substitute synthetic bracketed strings
  (`[Tool execution cancelled — ...]`,
  `[Result unavailable — see context summary above]`). Parse
  opportunistically, never assume valid JSON.
- **Deterministic fallback call ids**: when a provider omits a call id,
  hermes synthesizes `call_<sha256(name:args:index)[:12]>` (stable across
  retries by design, to keep prompt caches warm).
- **Skippable noise for import**: `tools[]` (unless tool-availability
  provenance is wanted — note the count varies, 28 vs 29 observed, as
  toolsets change), the duplicated `system_prompt`, and
  `encrypted_content` blobs (keep, don't parse).

## Known drift (source newer than corpus)

The repo at HEAD (`v2026.3.30-125-ge905768f`) writes shapes not present in
the observed corpus. `additionalProperties: true` already tolerates all of
them; listed so nobody mistakes them for corruption:

- `tool_calls[].extra_content` — provider continuity blobs (e.g. Gemini
  `thought_signature`), preserved round-trip.
- assistant `reasoning_details` — list of opaque provider dicts
  (OpenRouter/Anthropic-style reasoning continuity), preserved exactly.
- tool message `tool_name` — added when history is reconstructed from
  SQLite on resume (`hermes_state.get_messages_as_conversation`), so it
  can reach the JSON log of a resumed session.
- Pipe-joined ids `call_x|fc_y` exist in the transport layer
  (`_split_responses_tool_id`) as a legacy stored form; never observed
  on disk in this corpus.
- Transient bookkeeping keys on messages (e.g. `_flush_sentinel` on a
  synthetic memory-flush user message) are normally popped before save
  but exist in source; treat unknown keys as tolerable.
- The `_save_session_log` docstring reserves "injected system messages
  (compression summaries, todo snapshots)"; no `system`-role message was
  observed on disk, and the schema deliberately keeps `role` an open
  string so one would validate.

## Validation reproduction

Scratch validator (session-local):
`spoke-hermes/validate.py` under the session scratchpad — schema
self-check, all 12 documents, linkage + count invariants, request_dump
exclusion, and the 10 mutation tests. Result at derivation time:
12 pass / 0 fail / 0 invariant violations.
