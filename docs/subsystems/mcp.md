# MCP

This document covers the MCP subsystem:

- `bt-mcp`
- the MCP-facing parts of `bt-runtime`
- protocol/server/TUI surfaces that expose MCP state

The useful case-study lesson from both `pi` and Hermes is that MCP should be treated as a first-class subsystem, not an add-on bolted to a local tool registry.

## Purpose

MCP is Belltower's external tool extension boundary.

It should let Belltower:

- discover external tool servers
- manage their lifecycle
- surface their health and degraded state
- expose their tools through the same runtime path as built-in tools

## Supported Transport Modes

The stable target includes:

- stdio MCP servers
- streamable HTTP/SSE MCP servers

Important rule:

- transport differences should not leak into the rest of the harness more than necessary

## Runtime Model

MCP tools should be normalized into the same operational path as built-ins:

- same approval path
- same session events
- same export path
- same raw/structured telemetry expectations where applicable

This is crucial. If MCP becomes a parallel execution path, it will undermine the whole harness design.

The approval path includes the same durable request snapshot contract used for
built-in tools. When an MCP tool pauses for approval, Belltower stores the
request fingerprint, argument hash, redacted preview, tool metadata, initiator,
surface, and policy/registry evidence available at request time. The later
approval decision is recorded as a separate resolution. This keeps historical
MCP approvals inspectable even if the external server disappears, reloads with a
different schema, or changes its tool metadata.

## Degraded State Handling

MCP servers must have visible lifecycle state:

- configured
- discovered
- ready
- degraded

The operator should be able to see this in TUI, API, and inspection surfaces.

Current interpretation:

- `configured`: the server is known to the runtime but has not established a live session yet
- `discovered`: the transport/session is alive, but Belltower has not yet cached a tool inventory for the current runtime
- `ready`: Belltower has a live server and a cached discovered tool inventory
- `degraded`: the last MCP initialization, discovery, or call path failed and the reason is operator-visible

The current operator/query surfaces are:

- TUI `/mcp` and `/doctor`
- API routes for coherent MCP inventory, servers, tools, and reload
- `inspect({ query: "mcp" })` through the normal inspection tool path

Important rule:

- clients should have one coherent MCP inventory path when they need servers and tools together, instead of racing separate server and tool endpoints and reconstructing truth locally

## Scope Boundary

Belltower v1 supports MCP as the domain-extension boundary.

It does not require:

- generalized plugin hosting
- a broader extension runtime beyond MCP/custom tools

Those are explicitly separate concerns.

## Human Checkpoints

1. configure a simple stdio MCP server
2. verify it appears through `/mcp`, `/doctor`, or `inspect({ query: "mcp" })`
3. call one MCP tool from a session
4. replay the resulting session/tool events
5. repeat with streamable HTTP/SSE MCP when that path is complete

## Current Priorities

- deeper streamable HTTP/SSE coverage
- stronger catalogue ranking and MCP-heavy prompt exposure
- richer approval and timeout visibility on top of the current `/mcp` and inspection surfaces
