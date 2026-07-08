# Provider Integration Checklist

This is the Belltower-specific version of the best provider checklist pattern from `pi`.

Use this document whenever a provider is added or materially changed.

Related docs:

- [`../architecture/overview.md`](../architecture/overview.md)
- [`../planning/target-implementation-goals.md`](../planning/target-implementation-goals.md)
- [`../subsystems/providers-auth-and-models.md`](../subsystems/providers-auth-and-models.md)
- [`./provider-auth-compatibility-matrix.md`](./provider-auth-compatibility-matrix.md)
- [`../../AGENTS.md`](../../AGENTS.md)

## 1. Scope The Provider Honestly

Before writing code, decide and document:

- provider ID
- transport type
  - direct HTTP API
  - OpenAI-compatible API
  - stdio bridge
  - managed runtime bridge
- auth type
  - API key
  - OAuth
  - external auth broker
  - no auth
- feature surface
  - text streaming
  - tool calling
  - reasoning or thinking fields
  - images
  - prompt caching
  - usage reporting
- pricing metadata

Also decide whether the provider/auth pair is genuinely compatible with Belltower runtime behavior.
If the path is OAuth-backed or subscription-backed, check the compatibility matrix first and do not reuse an API-key provider path unless the runtime semantics are actually the same.

Do not declare support broader than the implementation actually provides.

## 2. Update Shared Types And Catalog Data

Check these surfaces:

1. `bt-core`
   - provider IDs
   - provider-facing shared types
   - auth capability types if needed

2. embedded data catalogs
   - `crates/bt-core/data/providers/catalog.toml`
   - pricing tables
   - context-window tables
   - default models or recommendation tables if applicable

3. config behavior
   - global config support
   - project config overrides
   - truthful default connection or model handling

## 3. Implement The Provider Path

Update `bt-providers` to cover:

- request validation
- request translation from Belltower messages to provider messages
- stream parsing
- tool-call translation into canonical tool-call events
- usage parsing
- finish-state mapping
- retry policy
- raw chunk capture for every streamed event or equivalent raw response unit

If the provider cannot persist raw output, it is not done.

## 4. Runtime And Protocol Surfaces

Check whether the provider changes:

- readiness validation in `bt-runtime`
- connection reporting in `bt-server`
- DTOs or provider state in `bt-protocol`
- client behavior in `bt-client`
- operator reporting in `belltower doctor` or `belltower status`

The launcher and server must tell the same truth.

## 5. Auth And Operator UX

Update the operator path if the provider requires:

- a new login flow
- a new logout or credential removal path
- different provider setup prompts
- different model selection behavior
- import or migration guidance

If the provider has non-key auth semantics, document them explicitly instead of pretending they are generic API-key auth.

## 6. Tooling And Context Semantics

If the provider changes tool behavior, check:

- tool-call delta assembly
- tool call and tool result pairing
- context token estimation or real tokenizer support
- compaction assumptions
- stop reasons and partial-output handling

## 7. Documentation

Update at least:

- [`../../README.md`](../../README.md)
- [`../subsystems/providers-auth-and-models.md`](../subsystems/providers-auth-and-models.md)
- [`../subsystems/telemetry-and-exports.md`](../subsystems/telemetry-and-exports.md) if telemetry semantics differ
- relevant quickstart or setup documentation

## 8. Required Tests

At minimum:

- request translation test
- validation test
- streaming parser test
- tool-call translation test if tool support exists
- usage parsing test
- raw chunk persistence test
- launcher/provider readiness truthfulness test if status output changes

For larger additions, also add:

- retry behavior tests
- malformed-stream tests
- context overflow tests
- cross-provider handoff tests if the provider will be used in mixed sessions

## 9. Human Acceptance Tests

A human should be able to run:

1. `belltower login <provider>`
2. `belltower doctor`
3. `belltower status --probe`
4. `belltower`
5. a simple prompt against that provider
6. a prompt that triggers tool use if tools are supported
7. a replay or raw-chunk inspection path

The expected result is not just “it answered.” The expected result is:

- readiness is truthful
- streaming works
- tool activity is visible if supported
- telemetry is persisted
- raw chunks are queryable

## 10. Done Criteria

A provider slice is done only when:

- the provider is wired through runtime, server, client, and launcher surfaces
- the support statement in docs is truthful
- raw output durability exists
- tests cover the parser and translation edge cases
- the human acceptance checks succeed
