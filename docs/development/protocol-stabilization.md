# Protocol Stabilization

Belltower's protocol is now under stabilization for the initial release path.

The `0.1.x` line is a preview protocol, not the first stable compatibility
line. Patch releases in that line preserve compatible route, DTO, replay, and
typed-error behavior. An intentional incompatible preview change requires a
minor-version bump, fixture updates, compatibility-test updates, and an
operator-facing migration note. The eventual first stable protocol will make
a stronger compatibility promise before release.

This rule lets the rest of the product move quickly without accidental
control-plane drift while making the current preview boundary explicit.

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

## Intentional Pre-Release Cutovers

Operator/control writes no longer infer a session's default branch.
`UpdateSessionRequest`, `UpdateSessionBudgetRequest`,
`CancelSessionRequest`, `SteerSessionRequest`,
`RecordOperatorCommandRequest`, and `RunShellCommandRequest` require
`branch_id`. This is an intentional narrow pre-release DTO cutover: preserving
omission would preserve ambiguous event attribution. Golden session-control
fixtures, generated OpenAPI, client call sites, and protocol-path tests pin the
new contract together.

The model-only `send_agent_message` contract makes the same cutover without an
OpenAPI DTO: `target_branch_id` is required for new messages, and replies must
reverse the immutable session-and-branch edge named by `in_reply_to`.
