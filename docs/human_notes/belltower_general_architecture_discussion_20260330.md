# Belltower: Architecture Discussion Summary

This document summarizes a comprehensive review of Belltower's architecture, competitive landscape, and strategic opportunities. It covers findings from reviewing Codex-rs, pi-mono, Hermes, Atropos, Honcho, Braintrust, Paperclip, and Karpathy's autoresearch — and how they inform Belltower's design.

---

## Architecture Fundamentals

- Codex-rs is excellent reference architecture but tightly coupled to OpenAI — not forkable, but worth studying (especially `codex-otel` and the compaction system).
- Belltower's "session is the telemetry record" is a genuine differentiator no other harness has.
- `bt-agent` purity is the most important single invariant — enforce at `Cargo.toml` level, never let it depend on session/server/runtime.
- The write path is sacred: construct event → commit to SQLite → mirror to tracing → broadcast to SSE. Never reverse this order.

## Traces Are the Product

- Every other harness treats traces as exhaust. Belltower treats them as the asset.
- The UI, the agent loop, the provider layer — all infrastructure to produce traces.
- The training loop, the memory pipeline, the export system — all consumers of traces.
- Everything follows from getting the trace quality right.

## Lossless Compaction

- Belltower gets lossless compaction for free from the append-only event log design.
- Compaction is a projection switch, not a data mutation — the prompt assembly query changes, the event log doesn't.
- Codex-rs mutates history in place — once compacted, the original conversation is gone forever.
- This directly enables:
  - Full session replay after compaction
  - Training on pre-compaction data
  - Diffing summaries against originals to improve compaction quality
  - Richer memory extraction from long sessions

## Memory and Compaction Are Separate Systems

- Compaction is within-session context window management (runtime-driven).
- Memory is cross-session learning (background pipeline: extract → store → rank → consolidate → inject).
- Codex-rs has both, implemented separately — worth studying their two-phase memory pipeline (per-session extraction, then global consolidation with usage-based ranking and decay).
- Honcho is a hosted alternative for the memory layer if you don't want to build extraction/consolidation from scratch.
- BT's structured event log gives richer memory extraction signal than conversation transcripts — it knows which tools succeeded, which files were modified, which patterns led to human intervention.

## OpenInference Alignment

- Structural, not nominal — use BT-owned field names, maintain a mapping table for OI export.
- This lets you update the OI mapping when the spec changes without migrating your SQLite schema.

## Multiplayer

This section has been promoted into the canonical architecture docs:

- [`../architecture/participants-and-collaboration.md`](../architecture/participants-and-collaboration.md)
- [`../architecture/signals-and-ingress.md`](../architecture/signals-and-ingress.md)
- [`../architecture/subagents-and-session-graphs.md`](../architecture/subagents-and-session-graphs.md)

- Owned by the harness, not the UI.
- Humans and agents are both "actors" sending events into sessions through the same protocol.
- Two humans collaborating, human supervising an agent, parent agent delegating to child agents, mixed human-agent teams — all the same pattern at the protocol level.
- Actor identity on events gives full provenance: who prompted, who approved, who steered.
- Deployment topology (hosted, shared server, Tailscale) is an ops concern, not an architecture concern.
- `belltower share` with short-lived invite tokens would make casual multiplayer practical.

## iOS / Mobile

- Thin client over SSE to a remote server works today with zero architecture changes.
- Local-on-device is possible: Rust compiles to iOS, SQLite is native, provider talks to local model on localhost.
- iOS sandbox constrains tools: no shell, but JSCore, document access, clipboard, Shortcuts, web fetch all work.
- The sweet spot is a personal reasoning agent, not a coding agent — research, analysis, drafting, planning, branching.
- Full trace quality identical to desktop — the infrastructure is the same, only the tool set differs.

## Autoresearch / Autonomous Loops

- The autoresearch pattern is a child session with tools, a budget, a scoring function, and a goal.
- Pi-autoresearch's JSONL sidecar is what BT does natively in the event log — no separate file needed.
- Lossless compaction means the agent can run for hundreds of experiments without losing experiment history.
- Karpathy's "swarm" vision maps to BT's session graph — each agent gets a child session with its own worktree.
- The ideas backlog maps to branches — defer a hypothesis by branching, explore it later.

## Cron / Scheduling

- Small addition to `bt-server`: cron_jobs table, tokio ticker, session creation on fire.
- Each cron job spawns a normal session with the full trace — not a special execution path.
- Results are inspectable, replayable, exportable, trainable — not just a markdown file in a directory.

## The RL / Training Pipeline (Atropos Connection)

- Traces from real usage → score them (automated: tool success, approval rates, efficiency) → group by similar tasks → GRPO makes good traces more likely → deploy updated model → better traces.
- Benchmark data for ground-truth scoring first, real session data for judgment-level training later.
- Raw stream durability is what makes this viable — you need token-level logprobs that only raw chunks preserve.
- Scoring is automated, not manual: tool exit codes, approval decisions, human interventions, turn counts, and retry patterns are all computable from the structured event log.
- `bt-improve` is the future bridge from BT's session store to Atropos-style training.

## Competitive Landscape

- **Braintrust / Langfuse / Phoenix** — observability consumers of traces, not competitors. BT could export to them.
- **Honcho** — complementary memory layer, possible integration point, but conflicts with "harness owns the data" principle.
- **Paperclip** — multi-agent orchestration layer above BT, could coordinate multiple BT sessions.
- **ACP** — editor integration protocol, not a replacement for `bt-protocol`. A thin adapter over `bt-server`. Add later if demand exists.

## Strategic Summary

Belltower's canonical session store with lossless compaction and unified actor model makes it the only harness where traces are an asset that compounds — through memory, training, multiplayer collaboration, and autonomous research loops — rather than exhaust that gets thrown away.

Build the moat first. Ship table stakes at whatever pace keeps the harness usable. Do not let feature pressure erode the architectural commitments that make the project worth doing.
