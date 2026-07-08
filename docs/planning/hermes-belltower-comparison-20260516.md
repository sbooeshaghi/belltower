# Belltower vs Hermes Agent Comparison

Date: 2026-05-16

## Scope

This review compares Belltower with the updated Hermes Agent case study checkout.
The goal is not to rank model quality. The goal is to identify useful harness
features, operator experience differences, benchmark readiness, and practical
lessons that matter for Belltower as scientific agent infrastructure.

Hermes was updated before review:

- Path: `docs/case-studies/harnesses/hermes-agent`
- Commit reviewed: `fb05f5d4b`
- Hermes version from the updated checkout: `v0.14.0 (2026.5.16)`

Belltower was reviewed from the current local workspace. The workspace already
contained active uncommitted web-retrieval foundation changes, so this comparison
does not claim a clean-release snapshot for Belltower implementation details.

## Commands Run

### Belltower

- `target/debug/belltower status`
  - Confirmed the active Belltower installation can see `chatgpt`, `local`, and
    other configured connections.
  - Confirmed ChatGPT readiness was healthy for the current auth store and the
    discovered model list included `gpt-5.4-mini`, `gpt-5.5`, `gpt-5.4`,
    `gpt-5.3-codex`, and `gpt-5.2`.
  - Confirmed web retrieval status currently defaults to `search=auto` and
    `fetch=direct_http`, with Exa auth missing.
- `bash scripts/tui_acceptance.sh deterministic`
  - Passed.
  - Covered `bt-tui` tests, minimum inspection contract reconstruction, and
    legacy-bundle/JSONL/HTML/ShareGPT/OTLP acceptance.
- `bash scripts/tui_acceptance.sh tmux`
  - Passed.
  - Exercised long TUI history, approval, `/inspect session`, `/compact`, and
    `/export jsonl`.
- `BELLTOWER_LIVE_ACCEPTANCE=1 bash scripts/live_acceptance.sh local`
  - Passed after Rust build warmup.
  - Observed the expected token `belltower-live-local-ok`.

### Hermes

- `git pull --ff-only`
  - Updated the case-study checkout to `fb05f5d4b`.
- `.../venv/bin/python ./hermes --version`
  - Reported `Hermes Agent v0.14.0 (2026.5.16)`.
- `.../venv/bin/python ./hermes status`
  - Reported local custom endpoint provider using `qwen3.5:latest`.
  - Reported OpenAI Codex OAuth logged in.
  - Reported most API-key providers absent in this environment.
- `.../venv/bin/python ./hermes -z "Reply with exactly this token and no extra text: hermes-live-local-ok"`
  - Returned `hermes-live-local-ok`.
  - The one-shot interface is convenient, but this run took noticeably longer
    than expected in this environment.
- `.../venv/bin/python -m pytest tests/plugins/web/test_web_search_provider_plugins.py -q`
  - Passed: `45 passed`.
- `.../venv/bin/python tests/stress/test_benchmarks.py`
  - Passed and wrote `/tmp/kanban_bench_results.json`.
  - Representative medians:
    - `dispatch_once` at 10k tasks: `12.9 ms`
    - `recompute_ready` at 10k tasks: `141.1 ms`
    - `build_worker_context` with 50 parents: `1.8 ms`
    - `list_tasks` at 10k tasks: `110.9 ms`
    - `board_stats` at 10k tasks: `6.3 ms`

## Feature Comparison

| Area | Belltower | Hermes Agent | Takeaway |
| --- | --- | --- | --- |
| Architecture | Rust workspace with explicit crates for protocol, session, runtime, tools, providers, server, TUI, readiness, auth, MCP, telemetry. | Python-first harness with CLI, gateway, TUI bridge, plugin system, memory, providers, batch runner, and many integrations. | Belltower has cleaner canonical seams. Hermes is broader and more productized. |
| Protocol boundary | HTTP/SSE protocol with `bt-protocol` DTO ownership and a server-first design. | CLI/gateway/API surfaces exist, but the core harness boundary is less singular. | Belltower is better positioned for strict clients and trace consumers. |
| TUI/operator UX | Recent Codex-style TUI work gives strong terminal history, active status, approvals, questions, inspection, and slash-command flow. | Mature CLI and modern TUI surface, plus one-shot and worktree-friendly options. | Belltower TUI is now competitive. Hermes still has stronger headless one-shot UX. |
| One-shot/batch usage | No equivalent first-class `belltower -z` or `belltower chat -q` path yet. Acceptance scripts cover deterministic and live checks. | `hermes -z` and `hermes chat -q` are first-class. `batch_runner.py` produces trajectory datasets. | Belltower should add a canonical headless run/eval path that records normal session telemetry. |
| Tools | Canonical built-in tools with approval policy, durable tool events, inspection, and export. Web retrieval foundation is in progress. | Broad tool catalog, plugin discovery, web search/extract/crawl, process/session tools, MCP, computer use, skills, memory. | Hermes has breadth. Belltower should keep narrower, canonical tool semantics and add high-value providers carefully. |
| Web retrieval | Moving toward first-class `web_search` and `web_fetch` with backend selection. | Mature `web_search`, `web_extract`, and `web_crawl` with provider plugins for Exa, Parallel, Tavily, Firecrawl, SearXNG, Brave, DDGS, and fallback logic. | Hermes is the stronger reference for provider pluggability. Belltower should copy the backend-selection idea, not the full surface area. |
| Auth/readiness | Structured readiness reasons, default fallback, ChatGPT OAuth support, provider/model discovery, local backend detection. | Extensive auth sources and provider status, including OpenAI Codex OAuth and many API-key providers. | Both are strong. Belltower's readiness is more canonical; Hermes exposes more provider integrations. |
| Sessions/telemetry | Canonical SQLite event store, raw chunks, branches, lineage, exports, OTLP/OpenInference, inspection contracts. | SQLite/JSONL sessions, exports, usage insights, Langfuse, trajectory conversion/compression. | Belltower has stronger audit-grade telemetry. Hermes is stronger for dataset/trajectory production today. |
| Memory/skills | Architecture docs cover memory and skills, but implementation is not yet core. | Built-in memory files, external memory providers, skill install/config/curation, curator workflow. | Hermes is ahead on memory/skills. Belltower should defer broad memory until context/provenance seams are mature. |
| Subagents/collaboration | Planning exists for subagents/session graphs and dogfooding, but design should remain conservative. | Worktree mode, Kanban collaboration board, subagent tests/progress, MCP serving. | Hermes has more collaboration machinery. Belltower should start from canonical session graphs and budgets before copying workflows. |
| Benchmarks | Acceptance scripts verify runtime/TUI/export contracts. Need a model-matched eval runner. | Stress tests and batch trajectory runner are concrete and easy to invoke. | Belltower needs a first-class eval harness that emits comparable metrics from canonical telemetry. |

## Practical Findings

### Belltower strengths

- The canonical event/session store is the main differentiator. It supports
  replay, inspection, export, and telemetry without treating the TUI as truth.
- The protocol boundary is cleaner than Hermes for external clients.
- The recent TUI work materially improves operator oversight. Inspecting tool
  calls while a turn is active is a strong operational feature.
- Export support is already broader than Hermes for trace interoperability:
  bundle, JSONL, HTML, ShareGPT, OTLP JSON, and OTLP protobuf.
- The provider/readiness path has a clear structured-contract direction.

### Hermes strengths

- One-shot usage is more ergonomic: `hermes -z` and `hermes chat -q` are
  obvious, scriptable entrypoints.
- Hermes has a much broader practical tool and integration ecosystem today.
- Web retrieval is more mature, especially backend discovery and per-capability
  provider selection.
- Memory and skills are productized enough to inspect as working systems.
- The Kanban/stress benchmarks and batch trajectory pipeline are useful
  references for future Belltower eval tooling.

### Belltower risks exposed by the comparison

- Belltower now has a first-class headless one-shot entrypoint:
  `belltower run --prompt ...`. This removes the need to drive tmux for simple
  benchmark/eval tasks, while keeping execution on normal session telemetry.
- Belltower's web retrieval direction is correct, but it should avoid becoming
  a generic search-product surface. Keep `web_search` and `web_fetch` simple,
  canonical, and provider-backed.
- Belltower's memory/skills/subagents plans should not copy Hermes' breadth
  until the core context, budget, provenance, and inspection seams are stable.
- Belltower has strong telemetry, but it needs a small number of operator-facing
  summaries that make traces easier to compare across harnesses.

## Recommended Belltower Follow-Ups

1. Harden the canonical headless run path.
   - Current shape: `belltower run --prompt ...`, `belltower run --prompt-file ...`,
     piped stdin, and optional `--json`.
   - Requirement: keep it on normal launcher/server/`bt-client`/runtime/session
     semantics. Do not add a direct runtime shortcut for benchmarks.
   - Next useful additions: streaming output, timeout controls, and optional
     existing-session continuation once those have clear operator use cases.

2. Add a small benchmark runner on top of canonical sessions.
   - Input: JSONL tasks with prompt, working directory, allowed connection/model,
     expected checks, and timeout.
   - Output: session id, pass/fail, elapsed time, tool count, approval count,
     token/cost summary, export path.
   - Why: Belltower should use its event store as the evaluation substrate.

3. Finish the web retrieval provider seam.
   - Keep Belltower's public tools narrow: `web_search` and `web_fetch`.
   - Add provider adapters behind them, starting with Exa and direct HTTP.
   - Use Hermes as the reference for backend availability and capability
     selection, not for expanding the tool vocabulary prematurely.

4. Add a trajectory export view.
   - Keep the canonical event store as source of truth.
   - Add a derived export that is easy to compare with Hermes-style trajectory
     JSONL and external benchmark datasets.
   - Why: scientific evals need both audit fidelity and easy downstream analysis.

5. Keep memory, skills, and subagents conservative.
   - Hermes proves these features are useful, but also shows how broad they can
     become.
   - Belltower should add them only through canonical context, session graph,
     approval, budget, and telemetry seams.

## Bottom Line

Hermes is currently broader and more immediately scriptable. Belltower is more
architecturally disciplined and better positioned for audit-grade scientific
telemetry.

The highest-leverage next step for Belltower is not copying every Hermes feature.
It is building benchmark and trajectory tooling on top of the canonical
headless/eval path so Belltower can be dogfooded and compared using the same
durable runtime/session semantics that make the project valuable.
