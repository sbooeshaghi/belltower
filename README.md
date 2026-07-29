# Belltower

Belltower is an open-source Rust agent harness for doing science with AI agents.

It is built for researchers and developers who need more than a chat wrapper:
durable traces, inspectable tool use, reproducible sessions, provider choice,
and a control surface that makes agent work understandable while it is running.

## Why Belltower

Most agent CLIs are optimized for one model provider or one interactive coding
workflow. Belltower is designed as reusable scientific infrastructure:

- **Durable session traces**: messages, tool calls, approvals, raw model output,
  costs, branches, and operator actions are written to a canonical SQLite event
  log.
- **Provider-neutral execution**: run local OpenAI-compatible models, OpenAI,
  Anthropic, ChatGPT subscription auth, and future providers behind one runtime
  and protocol boundary.
- **First-class tools**: file, shell, web, MCP, and operator commands flow
  through the same inspectable tool path instead of becoming hidden UI state.
- **Replay and forkability**: sessions are append-only histories that can be
  inspected, exported, resumed, and branched from earlier points.
- **Human-operable TUI**: the terminal UI exposes active work, history, queued
  input, approvals, and model/connection state without bypassing the server.
- **Extensible architecture**: `bt-protocol`, `bt-client`, `bt-server`,
  `bt-runtime`, and `bt-session` define clean seams for new clients, tools,
  telemetry exporters, and scientific workflows.

The core idea is simple: the harness is the product. Models change; the
orchestration, telemetry, tools, and session history are what make scientific
agent work reproducible and useful.

## Quick Start

From a source checkout:

```bash
cargo run -p belltower --
```

The launcher starts a local Belltower server when needed, creates or resumes a
session, and opens the TUI.

Authenticate a provider:

```bash
cargo run -p belltower -- login
```

Then use the TUI:

```text
/connection          # list or switch connections
/models              # list models for the current connection
/use <conn> <model>  # switch this session
/defaults ...        # set future-session defaults
/help                # list commands
```

Headless one-shot mode uses the same runtime, protocol client, and session store
as the TUI:

```bash
cargo run -p belltower -- run --prompt "Summarize this project"
```

## Runtime Support

Supported now:

- OpenAI-compatible APIs
- ChatGPT subscription OAuth
- Anthropic APIs
- local OpenAI-compatible backends such as Ollama
- Exa-backed `web_search` and direct HTTP `web_fetch`

Planned:

- Google Vertex
- richer session bundle publishing and synchronization
- deeper scientific database and evidence-card workflows

## Install

- Cargo from a source checkout: `cargo install --path crates/bt-server && cargo install --path crates/bt-tui && cargo install --path crates/belltower`
- Release archive: download the matching `belltower-<version>-<target>` archive
  from GitHub Releases and keep `belltower`, `bt-server`, and `bt-tui` in the
  same directory on your `PATH`.
- Source checkout: `cargo run -p belltower --`

Version 0.1 deliberately does not publish workspace crates to crates.io.
The supported distribution paths are GitHub release archives and a source
checkout; the workspace crates are implementation details, not a public Rust
crate API.

See [`docs/install.md`](./docs/install.md) for platform notes and verification
steps.

## Development

Useful checks:

```bash
cargo fmt --all
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Workspace conventions:

- Rust edition `2024`
- Cargo `resolver = "2"`
- shared workspace dependencies
- SQLite-backed local storage via `rusqlite`
- `#![forbid(unsafe_code)]` in every crate

## Documentation

- docs index: [`docs/README.md`](./docs/README.md)
- install guide: [`docs/install.md`](./docs/install.md)
- architecture overview: [`docs/architecture/overview.md`](./docs/architecture/overview.md)
- session bundles and trace sync: [`docs/architecture/session-bundles-and-trace-sync.md`](./docs/architecture/session-bundles-and-trace-sync.md)
- subsystem map: [`docs/planning/subsystem-parity-matrix.md`](./docs/planning/subsystem-parity-matrix.md)
- development rules: [`AGENTS.md`](./AGENTS.md)

## License

[MIT](./LICENSE)
