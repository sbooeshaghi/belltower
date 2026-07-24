# Providers, Auth, and Models

This document covers:

- `bt-providers`
- `bt-auth`
- `bt-models`
- the provider/model/auth parts of `belltower`

The closest `pi` analogue is `@mariozechner/pi-ai`, while Hermes provides the best operator UX patterns for setup, login, and model selection.

## Purpose

This layer makes Belltower actually usable with real model backends.

It is responsible for:

- provider execution and validation
- credential storage and resolution
- local model/backend discovery
- truthful capability reporting in launcher and TUI surfaces

## Provider Model

Belltower currently distinguishes between:

- cloud providers with key-oriented auth
- ChatGPT/Codex subscription-backed runtime via a distinct provider path
- local OpenAI-compatible backends
- planned providers that must be surfaced honestly as planned

The important rule is:

- support is not just "config exists"
- support means the runtime can validate and use the connection

## Auth Model

The auth layer should remain explicit and inspectable.

Current shape:

- Belltower-owned auth store
- pluggable credential-store seam inside `bt-auth`, with file storage as
  the current default plus ephemeral-env and feature-gated keychain
  backends available behind the same resolver contract
- connection-scoped credentials
- connection-level auth method descriptors
- refreshable OAuth token bundles with account metadata in the auth store
- a distinct `chatgpt` device-code login path that stores refreshable OAuth bundles without pretending they are OpenAI API keys
- refresh-aware runtime credential resolution for ChatGPT so readiness and execution use a live access token instead of stale store state; access tokens are refreshed before use when near expiry, when expiry metadata is missing, or when the token shape is unreadable while a refresh token is stored
- automatic refresh updates the credential backend that supplied the stored bundle, so an auto-mode file credential is not refreshed into keychain while leaving a single-use stale refresh token behind in `auth.json`
- if two Belltower processes race to refresh the same single-use ChatGPT refresh token, the losing process reloads the auth store and uses the newer fresh credential when one was already written
- refresh failures are reported as structured non-ready connection state and must not abort inspection or launcher startup for unrelated usable connections such as `local`
- ChatGPT model discovery pinned to the backend's Codex compatibility `client_version` so Belltower sees the same subscription-visible model set the upstream runtime exposes
- provider construction through runtime credential shapes rather than raw API-key plumbing alone
- literal, env-based, and command-based secret references
- structured readiness states on `ConnectionReadinessInspection` / `ConnectionModelInventory`, with launcher and TUI consuming those states instead of inferring categories from `probe_ready` plus freeform status text
- explicit unsupported-auth-method reporting when a configured credential cannot actually drive the current runtime path
- truthful credential source reporting in launcher `doctor` and `status`
- TUI-native readiness inspection through `/status` and `/doctor`
- connection-scoped model inventories through `/models`
- targeted connection model inspection through `/models/connections/{connection_id}`
  for model pickers and `/models <connection>`
  so simple operator flows do not probe unrelated providers
- startup-safe readiness for implicit remote defaults: launcher startup may
  bound the selected default connection probe and fall back to `local`, but
  full readiness/model inventory truth remains centralized in `bt-readiness`
  and is surfaced through TUI/status/model commands after launch
- runtime turn execution uses the same provider/auth compatibility seam before
  recording model-visible context mutations; failed provider construction or
  credential preflight is a durable turn failure, not a successful context
  preparation
- provider-specific curation on top of raw remote model discovery so operator-facing inventories only surface the relevant supported families for that connection
- discoverable-model inventories that do not silently inject the configured default back into the runtime list when the backend did not expose it
- quiet high-thinking defaults for models with known reasoning controls:
  the context/request layer enables model-side reasoning effort for supported
  OpenAI, ChatGPT/Codex, and Anthropic models without adding a primary operator
  option; provider adapters translate that canonical request intent into each
  backend's request shape. Model capability predicates (reasoning family,
  default/maximum effort, thinking support) are centralized in
  `bt_core::model_capability` and must not fork per crate. Defaults stay
  within what each endpoint accepts: `xhigh` only for codex-max variants,
  and the OpenAI chat-completions adapter clamps `xhigh` to `high` because
  that endpoint rejects it (observed 400 on gpt-5.4)
- auth-method labels carried directly on readiness and model inspection surfaces so launcher, CLI, and TUI can render supported login paths from canonical state
- one shared HTTP client (connection pool) behind every provider instance,
  with connect/read-stall timeouts but never a total request timeout that
  could kill a long streaming completion; providers are cheap to construct
  per turn because the pool outlives them
- OAuth refresh is serialized per provider with a re-check under the lock:
  rotating refresh tokens (ChatGPT) must never be spent twice by racing
  turns, which permanently burns the stored token
- guided connection-plus-model selection through `/use`

Future shape:

- import helpers and migration helpers
- operator-facing auth-storage selection and precedence reporting on top
  of the now-pluggable store seam
- connection-level auth method descriptors and runtime credential seams so non-key flows have an explicit place to land
- later, runtime-complete separate auth strategies for non-API-key flows where those are truly supported
- an explicit compatibility matrix that blocks fake subscription/API-key equivalence
- operator login/setup flows that choose an auth method honestly instead of assuming every remote connection is API-key based

Subscription-backed Claude (`claude-code` connection, provider `claude-cli`):

- runs completions by subprocessing the operator's locally installed and
  logged-in Claude Code CLI (`claude -p --output-format stream-json`);
  belltower never sees or stores credentials, and ambient
  `ANTHROPIC_API_KEY`-style env vars are stripped from the child process so
  usage always bills the subscription
- the CLI's `stream_event` payloads are Anthropic Messages events, so
  translation reuses the anthropic provider's stream state machine; every
  NDJSON envelope is persisted as a raw chunk
- v1 is completion-only: declared harness tool specs are not bridged into the
  CLI (a warning is recorded and the model has no callable tools on this
  connection), the CLI's own tools are disallowed and the loop is capped at
  one turn, and structured output is unsupported — use the `anthropic` API
  connection for tool-calling sessions until an MCP bridge lands
- readiness reports Degraded when the `claude` binary (override:
  `BELLTOWER_CLAUDE_BIN`) is missing or unresponsive; auth problems surface
  on the first completion as a durable session error rather than a quota-
  spending readiness probe

Important rule:

- incompatible tokens must not be presented as valid provider auth
- a subscription-backed runtime path is only supportable through its own provider/runtime implementation (the `claude-cli` provider is exactly this)
- provider-specific capabilities like structured output must be surfaced truthfully and should not become first-class operator flows before the underlying auth/model/readiness UX is strong
- request-side capability policy should remain centralized and quiet until the
  operator experience is proven. Thinking/reasoning defaults are a provider
  request concern, not a slash-command or TUI setting by default.

## Provider Validation

Every implemented provider should support:

- connection validation
- completion execution
- model inventory where the upstream supports it
- token/cost normalization where possible
- raw stream durability

This validation path must feed:

- `belltower doctor`
- `belltower status --probe`
- TUI `/status`
- setup-time readiness checks

Runtime session transitions must also reject unconfigured connection IDs before
creating a session or writing a settings revision. Dynamic readiness and model
availability remain provider/readiness concerns, but the configured connection
catalog is runtime truth for session state.

Important rule:

- readiness detail may stay human-readable, but the blocked/ready reason class must stay structured and centralized in `bt-readiness`
- when runtime model inventory is discoverable and non-empty, `bt-readiness` should surface configured-model unavailability centrally instead of leaving launcher or TUI surfaces to infer it
- readiness may use bounded or asynchronous probes for startup usability, but
  the runtime provider-construction gate is still authoritative for whether a
  turn can enter context preparation and provider execution

## Local Models

Local inference is a first-class part of Belltower's product boundary.

At minimum Belltower should detect:

- Ollama
- LM Studio
- llama.cpp-compatible servers

Truthfulness rules:

- the configured local connection must map to a real backend
- backend reachability is not enough
- the configured default model must also exist on that backend
- for remote providers, configured-model unavailability should only be surfaced when the runtime can actually discover a non-empty model inventory; failed or empty discovery is not strong enough evidence on its own

This is a place where the launcher must stay strict rather than optimistic.

## Design Commitments

- provider support is centralized
- auth resolution is centralized
- local capability reporting is truthful
- provider catalog and pricing/context data stay data-driven
- curated connection model selectors and fallbacks stay small, explicit, and catalog-driven
- discovered models should come from the connection or backend at runtime, not manual registration
- connection-scoped model inventories should treat runtime discovery as canonical when discovery is available
- the configured default model may influence preference order, but it must not appear as discoverable inventory unless the backend actually exposed it
- persisted future-session defaults should only be accepted from the discoverable inventory when discovery is available
- user-defined custom connections remain the escape hatch for private or institutional endpoints

## Human Checkpoints

1. `belltower login openai`
2. `belltower login chatgpt`
3. `belltower doctor`
4. `belltower status --probe`
5. `belltower setup --connection local`
6. verify that local readiness changes when the configured model is not actually present

## Current Priorities

The next improvements for this layer are:

- provider/auth compatibility gating for additional subscription-backed work beyond ChatGPT
- shared auth-method and runtime-credential seams for the next non-key provider
- auth import/migration helpers
- deeper custom-endpoint discovery and guidance on top of the current `/status` / `/models` / `/doctor` surfaces
- more provider conformance coverage
- richer local model lifecycle actions beyond discovery, building on `/models`
- deeper `/use` guidance for blocked or misconfigured connections before send
