# Compaction and Reasoning Opacity: Study and Decision

Date: 2026-07-25. Sources: codex source at tag rust-v0.137.0 (matches the
installed CLI) + the local corpus's 2,470 real `compacted` records; letta +
letta-code source at HEAD; belltower's `bt-context` compaction and provider
stream/replay paths. The reasoning capture/replay fixes this study motivated
landed as commit `bb20bd3`.

## 1. How codex compacts

- **Trigger**: 90% of the model's context window, computed from
  provider-reported usage on the last response plus a bytes/4 estimate for
  items appended since — no client tokenizer. Checked pre-turn, mid-turn
  (only when a follow-up request is pending), on manual `/compact`, and on
  model downshift (compacting under the *old* model's window before
  switching to a smaller one).
- **Algorithm** (shipped default at 0.137): summarize the whole history via
  a "CONTEXT CHECKPOINT COMPACTION … handoff summary for another LLM"
  prompt; replacement history = **real user messages verbatim within a
  token budget** (20k local / 64k remote, newest-first) + the summary
  **last**, "as the model is trained to expect". Developer instructions are
  re-injected. Tool calls/outputs and all reasoning items are dropped from
  replayable history (the append-only rollout keeps everything — their
  evidence/context split matches ours).
- **Composition is a chain, not a stack**: the previous summary is included
  in what gets summarized and excluded from what gets retained, so
  successive compactions fold into one summary and never accrete. On disk
  this appears as the window-generation chain we measured empirically:
  `window_number` / `previous_window_id` / `first_window_id`, up to 264
  windows in one real session.
- **Caveat we must not copy**: on the ChatGPT backend (remote v2), the
  rolling summary is a provider-**encrypted** blob and the visible
  `message` is empty — the operator cannot read their own compaction
  record, and it is account-locked.

## 2. How letta compacts

The user's hypothesis is confirmed by letta's own source: the 90% proactive
threshold docstring says it "aligns … with the codex harness' proactive 90%
compaction policy", and the default `sliding_window` mode is codex-shaped —
evict-from-front to a cutoff that must land on an assistant boundary, keep
the entire recent tail verbatim (all roles), summarize the evicted span with
a **separate cheap model** (haiku/gpt-5-mini class), place the summary as a
first-class `role=summary` message carrying stats (trigger, tokens
before/after), recursive-by-eviction composition.

What letta adds that codex lacks — and where belltower should look:

- **Nothing is destroyed**: context is a pointer list (`message_ids`) over
  an immutable message store; eviction drops pointers. This is exactly
  belltower's event-log invariant, arrived at independently.
- **Summaries index the archive instead of replacing it**: the prompt's
  "Lookup hints" section ("note the topic and key terms that could be used
  to find it in message history later") pairs with `conversation_search`
  over evicted history.
- **Protected structural pairs**: whole-message eviction units; an approval
  request and its same-step assistant message (thinking + tool_calls) are
  never split.
- **Provenance-tagged encrypted reasoning types** ("so that we don't
  attempt … to put Anthropic encrypted reasoning into a GPT-5 responses
  payload") — the same rule our provider tags now enforce.

## 3. Belltower today, and the decision

Current compaction is deterministic-extractive: heuristic estimate against
`context_windows.toml` minus reserve, drop oldest units, inject a rule-based
digest (file activity + dropped content) as a System message, recorded
canonically as `context.compacted`. Honest, replayable, cheap — but the
summary quality ceiling is far below a model-written checkpoint, and the
trigger never sees real provider usage.

**Decision: adopt the codex/letta sliding-window shape, on belltower's
canonical substrate.** Belltower is structurally ahead on the part both of
them had to build around (nothing is ever destroyed — the event log is the
immutable store; compaction is already only a context-selection change plus
a canonical event). What we adopt is the policy and the summary quality:

1. **Trigger**: proactive at a configurable fraction (default 0.9) of the
   model window, computed from the last `completion.finished` usage plus a
   bytes/4 estimate of events appended since; checked pre-turn and mid-turn
   before follow-up sampling; manual `/compact` retained; model-downshift
   compaction on connection/model switch to a smaller window.
2. **Retention**: keep the recent tail verbatim with cutoffs only at
   assistant-message boundaries (letta), and always retain real user
   messages within a token budget (codex) — never split tool_use/result or
   approval/assistant pairs.
3. **Summary**: model-written via a summarization LLM call that runs as a
   normal recorded turn step — prompt derived from codex's checkpoint
   prompt plus letta's "preserve identifiers verbatim" and "lookup hints"
   sections (hints feed belltower's existing session search). Configurable
   cheap summarizer model per connection with same-model fallback.
   **Summaries are always plaintext evidence** — the encrypted-summary
   design is explicitly rejected.
4. **Composition**: chain, not stack — the prior summary is folded into the
   next and excluded from retention.
5. **Provenance**: `context.compacted` gains window-chain attributes
   (window number, previous/first window refs) mirroring codex's on-disk
   model; tokens before/after and trigger are already recorded.
6. **Reasoning at the boundary**: opaque replay parts are dropped from the
   *selected context* at compaction (providers only require same-turn
   preservation); the canonical log keeps them as evidence, and the
   provider tags prevent cross-provider replay regardless.
7. **Local models**: same policy; correctness comes from real window
   metadata — enrich `context_windows.toml` from ollama `/api/show` where
   available (letta's approach) instead of the 32k fallback.

Avoid, explicitly: letta's four-mode/three-fallback sprawl (one policy),
encrypted summaries (codex remote v2), compaction that silently discards
tool evidence from the only record (codex rollouts survive only via side
events; our canonical log already fixes this).

## 4. Reasoning opacity: mechanics and harness impact

Three regimes, now all handled (commit `bb20bd3`):

- **OpenAI/ChatGPT encrypted reasoning**: with `store: false` the backend
  returns reasoning items only when asked
  (`include: ["reasoning.encrypted_content"]`) and expects them replayed
  verbatim on subsequent requests; codex replays them on every request with
  no model guards and drops them only at compaction. Belltower now
  requests, captures (verbatim item, provider-tagged), and replays them in
  part order. Live-verified through a real gpt-5.4-mini tool loop.
- **Anthropic signed thinking**: thinking blocks stream with a
  `signature_delta` and must be replayed unmodified within same-turn tool
  loops; `redacted_thinking` is opaque data. Belltower now accumulates
  text+signature per block, closes each as a discrete signed replay part,
  and replays signed blocks (only when thinking is enabled, only
  provider-matched). Unsigned blocks stay display-only. This fixes a latent
  hard breakage for keyed anthropic tool sessions.
- **Local models**: plaintext reasoning in vendor-specific delta fields
  (`reasoning` on ollama, `reasoning_content` on DeepSeek/vLLM) — now
  captured as reasoning; no replay constraints exist, and none is sent
  back (correct per those APIs). Verified live against qwen3.5.

Harness rules that fall out (also encoded in the spoke-schema notes): opaque
reasoning is provider-scoped and never crosses providers; it never survives
egress to another harness (cannot be honestly synthesized); it is evidence
in the canonical log forever regardless of what context selection drops;
and token accounting for reasoning comes from usage details, not content.

## 5. Implementation order (when approved)

1. Trigger rework (provider-usage + estimate, 0.9 fraction, mid-turn check,
   downshift) — `bt-context`/orchestrator seam, config knobs.
2. Summarization turn + prompt + cheap-model config; `context.compacted`
   window-chain attributes.
3. Retention policy (assistant-boundary tail + verbatim user budget +
   protected pairs) replacing the extractive unit-dropper; keep the
   deterministic digest as the no-summarizer fallback.
4. Ollama window enrichment.
