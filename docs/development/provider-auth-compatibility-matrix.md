# Provider Auth Compatibility Matrix

Date: 2026-04-13

This document is the implementation gate for provider auth work.

Use it before adding:

- OAuth login flows
- subscription-backed auth
- auth import helpers
- provider/runtime support claims

Related docs:

- [`./auth-migration-and-imports.md`](./auth-migration-and-imports.md)
- [`./provider-integration-checklist.md`](./provider-integration-checklist.md)
- [`../subsystems/providers-auth-and-models.md`](../subsystems/providers-auth-and-models.md)
- [`../planning/target-implementation-goals.md`](../planning/target-implementation-goals.md)

## Rules

- Credential presence is not provider compatibility.
- Subscription-backed auth must not be overloaded into an API-key provider path.
- A provider/auth pair is only supportable when Belltower can:
  - store the credential in its own auth store
  - validate the connection through the real runtime path
  - list or otherwise truthfully report usable models
  - execute a real streamed completion
  - preserve raw output durability
- If any of those are missing, the auth path is not ready to ship.

## Current Belltower Ground Truth

Today Belltower runtime support is:

- `openai-compatible` with API-key auth
- `anthropic` with API-key auth
- `openai-chatgpt` with ChatGPT device-code OAuth auth
- local OpenAI-compatible backends with no auth

Current non-key support is now real for the first subscription-backed provider:

- `CredentialKind::OAuthToken` exists in shared types
- `bt-auth` can persist refreshable non-key credentials plus account metadata
- `belltower login chatgpt` now performs a distinct ChatGPT device-code auth flow and stores the resulting OAuth bundle in Belltower's auth store
- `bt-auth` refreshes expiring ChatGPT access tokens on the runtime/readiness path before provider construction
- `bt-providers` implements the real ChatGPT Codex runtime path against `/backend-api/codex/models` and `/backend-api/codex/responses`
- ChatGPT model discovery uses the backend's Codex compatibility `client_version`, not Belltower's package version, so `/models` and `/use` reflect the models the subscription runtime actually exposes
- `bt-readiness` validates the ChatGPT runtime path and reports discovered models through the same canonical readiness/model inventory surfaces as other providers
- unsupported configured auth methods are blocked explicitly in readiness rather than falling through ambiguous validation behavior

That means Belltower now has one distinct, runtime-complete subscription-backed provider path. The important constraint still stands: that support comes from a separate provider/runtime implementation, not from pretending a subscription token is an API key.

## Compatibility Matrix

| Provider/Auth Path | Current Status | Ship Now | Why |
| --- | --- | --- | --- |
| OpenAI-compatible + API key | Implemented | yes | Matches runtime and readiness behavior today |
| Anthropic + API key | Implemented | yes | Matches runtime and readiness behavior today |
| Local OpenAI-compatible + no auth | Implemented | yes | Runtime behavior is local and explicit |
| Vertex + Google ADC | Planned | no | Catalog exists, runtime path not implemented |
| ChatGPT + device-code OAuth login | Implemented | yes | Distinct provider/runtime path validates, discovers models, refreshes tokens, and streams real completions |
| ChatGPT/Codex subscription token treated as OpenAI API key | Incompatible | no | Different product/billing/runtime semantics |
| Claude subscription or Claude Code token treated as Anthropic API key | Incompatible | no | Different product/billing/runtime semantics |
| Gemini OAuth/device auth | Unproven | no | Provider/runtime path not implemented |
| Qwen OAuth/device auth | Unproven | no | Provider/runtime path not implemented |

## First Supported Target Rule

The first subscription-backed auth target must satisfy both conditions:

1. it uses a distinct Belltower provider or auth strategy rather than the generic API-key provider path
2. the runtime contract is proven with a real validation, model inventory, and streaming completion path

The first supported subscription-backed target is now:

- ChatGPT via the distinct `openai-chatgpt` provider and device-code OAuth

This is intentional. It keeps `login`, `status`, `doctor`, launcher surfaces, and TUI surfaces truthful by tying support claims to the actual runtime path.

## Phase Gate Checklist

Before moving a provider/auth path from "unproven" to "supported", capture:

1. token source and refresh behavior
2. exact request headers and transport shape
3. model inventory behavior
4. streaming completion behavior
5. raw output durability behavior
6. operator-facing readiness states for:
   - missing auth
   - expired auth
   - refresh failure
   - entitlement mismatch
   - capability mismatch

If any item remains speculative, the path stays blocked.
