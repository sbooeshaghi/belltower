# Belltower - Project Summary

## Overview
**Belltower** is a standalone Rust agent harness whose typed SQLite event log
is the source of truth behind a canonical HTTP/SSE protocol boundary. The goal
is a reproducible, shareable, extensible execution record rather than a UI-local
chat transcript.

## Core Identity
- **Language**: Rust (edition 2024)
- **Architecture**: Workspace-based multi-crate project
- **Storage**: SQLite-backed local storage (rusqlite)
- **Telemetry**: OpenInference-aligned with OpenTelemetry export
- **Primary evidence**: committed typed events; telemetry and UI state are derived consumers

## Workspace Structure
The project contains 18 crates in `/crates/`:

### Foundation Crates
- `belltower` - User-facing launcher and operator CLI
- `bt-core` - Core foundation
- `bt-protocol` - Protocol definitions
- `bt-client` - HTTP client layer
- `bt-server` - HTTP/SSE server
- `bt-auth` - Authentication

### Runtime & Agent Crates
- `bt-session` - Session management
- `bt-runtime` - Runtime execution
- `bt-agent` - Agent implementation
- `bt-context` - Context management
- `bt-readiness` - Readiness checks

### Model & Provider Crates
- `bt-models` - Model abstractions
- `bt-providers` - Provider integration

### UI & Tooling Crates
- `bt-tui` - Terminal UI
- `bt-tools` - Utility tools
- `bt-mcp` - MCP protocol support
- `bt-improve` - Self-improvement logic
- `bt-otel` - OpenTelemetry export

## Supported AI APIs
- ✅ OpenAI-compatible APIs
- ✅ Anthropic APIs
- ✅ Local backends (Ollama, etc.)
- 🔄 Google Vertex (planned)

## Key Features
- **Protocol Normalization**: Standardized HTTP/SSE boundary
- **Event Taxonomy**: Structured event model for telemetry
- **Tool System**: Normalized tool interfaces
- **Subagent Architecture**: same-lineage session graphs, explicit branch-routed durable messaging, exact reply edges, and per-child connection/model selection
- **Telemetry**: OpenInference-aligned exports derived from canonical events
- **Replay and Sharing**: portable session bundles, branching, inspection, and restart reconstruction
- **TUI Interface**: Terminal-based user interface
- **Authentication**: Built-in auth management (`belltower login`, `belltower logout`)

## Quick Start
```bash
# Build
cargo check --workspace
cargo fmt --all
cargo test --workspace

# Run
belltower                          # Setup + TUI
belltower login                    # Authenticate
belltower model                    # Manage model
belltower doctor                   # Health check
belltower status                   # System status
```

## Documentation
- [`docs/`](docs/) - Main documentation index
- [`docs/architecture/`](docs/architecture/) - System architecture
- [`docs/planning/`](docs/planning/) - Development targets & roadmap
- [`docs/development/`](docs/development/) - Development procedures
- [`docs/subsystems/`](docs/subsystems/) - Subsystem specifications

## Repository
- GitHub: https://github.com/sbooeshaghi/belltower
- License: [`MIT`](LICENSE)
