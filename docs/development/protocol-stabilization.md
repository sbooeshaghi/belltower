# Protocol Stabilization

Belltower's protocol is now under stabilization for the initial release path.

This is not a claim of permanent backward compatibility.
It is an internal development rule so the rest of the product can move quickly without accidental control-plane drift.

## What Is Stabilized

The current stabilization boundary is:

- protocol header and version negotiation
- typed error envelope
- HTTP route names and auth behavior
- SSE envelope shape and replay semantics
- top-level request/response DTOs for:
  - session control
  - workflow and inspection
  - readiness and models
  - MCP
  - export

## What A Change Requires

If a change affects route names, DTO shape, error envelopes, SSE envelopes, or protocol header behavior, it must also include:

1. fixture updates in `crates/bt-protocol/tests/fixtures/`
2. compatibility-test updates in `bt-protocol`, `bt-client`, or `bt-server`
3. doc updates if the change is user-visible

## What Is Still Allowed

Because Belltower is still pre-release, cleanup is still allowed.

But it must be:

- intentional
- narrow
- done before or together with fixture updates

This is why the stabilization harness exists:

- so TUI, auth/models, MCP, and export work can keep moving
- without re-litigating the core control plane every time
