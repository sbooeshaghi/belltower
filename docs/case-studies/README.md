# Case Studies

This directory tracks external systems and references that informed Belltower's
architecture, subsystem design, and usability review work.

It contains two kinds of material:

- local mirrors of relevant upstream repositories
- curated external references, including papers and product or standards docs

The mirrored repositories are intentionally kept as local working clones and are
ignored by the parent Belltower Git repo. The tracked surface in this directory
should stay limited to curated indexes, notes, and mirrored reference artifacts
such as PDFs.

Reference material should follow a simple convention:

- each category may add a `papers/README.md` index
- mirrored PDFs should live under `papers/pdfs/`
- indexes should clearly distinguish:
  - local mirrors
  - link-only references
  - unresolved or access-restricted sources

## Layout

- `harnesses/`
  - coding-agent and terminal-harness systems
- `memory/`
  - memory systems and memory-specific references
- `telemetry/`
  - tracing, observability, and export consumers or standards
- `orchestration/`
  - higher-level multi-agent orchestration systems
- `protocols/`
  - protocol and client/server integration references
- `training/`
  - RL and improvement-loop systems
- `research/`
  - autonomous research-loop references

## Local Repo Mirrors

### Harnesses

| Local folder | Upstream | Notes |
| --- | --- | --- |
| `harnesses/codex` | `openai/codex` | Canonical Codex OSS repo. |
| `harnesses/code` | `just-every/code` | Maintained fork of Codex used in the earlier survey. |
| `harnesses/opencode` | `sst/opencode` | Canonical OpenCode repo. |
| `harnesses/pi-mono` | `badlogic/pi-mono` | Canonical `pi-mono` repo. |
| `harnesses/oh-my-pi` | `can1357/oh-my-pi` | Exact-name public match; keep treated as lower-confidence than `pi-mono`. |
| `harnesses/goose` | `block/goose` | Canonical Goose repo. |
| `harnesses/gemini-cli` | `google-gemini/gemini-cli` | Canonical Gemini CLI repo. |
| `harnesses/qwen-code` | `QwenLM/qwen-code` | Canonical Qwen Code repo. |
| `harnesses/grok-cli` | `whitesmith/grok-cli` | Public Grok CLI wrapper; not an xAI-authored source repo. |
| `harnesses/hermes-agent` | `NousResearch/hermes-agent` | Matches the doc paths used in the survey. |
| `harnesses/aider` | `Aider-AI/aider` | Canonical Aider repo. |
| `harnesses/Warp` | `warpdotdev/Warp` | Public Warp repo used as an outlier reference. |
| `harnesses/agent-of-empires` | `njbrake/agent-of-empires` | Exact-name public match; keep treated as lower-confidence. |
| `harnesses/mistral-vibe` | `mistralai/mistral-vibe` | Canonical Mistral Vibe repo. |
| `harnesses/awesome-agent-harness` | `AutoJunjie/awesome-agent-harness` | Exact-name public match; keep treated as lower-confidence. |

Unresolved:

- `claude-code`
  - no public GitHub source repository was verified during this rebuild, so it
    is intentionally not mirrored here

### Memory

| Local folder | Upstream | Notes |
| --- | --- | --- |
| `memory/honcho` | `plastic-labs/honcho` | Memory-layer reference named in Belltower docs. |

### Telemetry

| Local folder | Upstream | Notes |
| --- | --- | --- |
| `telemetry/langfuse` | `langfuse/langfuse` | Observability consumer of traces. |
| `telemetry/phoenix` | `Arize-ai/phoenix` | Observability and eval surface. |
| `telemetry/openinference` | `Arize-ai/openinference` | Semantic conventions and instrumentation. |
| `telemetry/braintrust-sdk` | `braintrustdata/braintrust-sdk` | Trace consumer and eval tooling. |
| `telemetry/opentelemetry-specification` | `open-telemetry/opentelemetry-specification` | OTel spec source. |
| `telemetry/opentelemetry-proto` | `open-telemetry/opentelemetry-proto` | OTLP protobuf definitions. |

### Orchestration

| Local folder | Upstream | Notes |
| --- | --- | --- |
| `orchestration/paperclip` | `paperclipai/paperclip` | Multi-agent orchestration reference named in Belltower docs. |

### Protocols

| Local folder | Upstream | Notes |
| --- | --- | --- |
| `protocols/agent-client-protocol` | `agentclientprotocol/agent-client-protocol` | ACP reference. |

### Training

| Local folder | Upstream | Notes |
| --- | --- | --- |
| `training/atropos` | `NousResearch/atropos` | RL environment and trajectory system referenced in docs. |

### Research

| Local folder | Upstream | Notes |
| --- | --- | --- |
| `research/autoresearch` | `karpathy/autoresearch` | Autonomous research-loop reference. |

## Paper And Reference Index

Current paper/reference index:

- [`memory/papers/README.md`](./memory/papers/README.md)

This should stay curated. Add links with short notes explaining why they matter
to Belltower rather than dumping an unfiltered bibliography.
