# Auth Migration and Import Strategy

Belltower needs a first-class auth subsystem, but it must remain honest about what credentials mean.

Related docs:

- [`../subsystems/providers-auth-and-models.md`](../subsystems/providers-auth-and-models.md)
- [`../planning/target-implementation-goals.md`](../planning/target-implementation-goals.md)
- [`./auth-storage.md`](./auth-storage.md)
- [`./provider-auth-compatibility-matrix.md`](./provider-auth-compatibility-matrix.md)
- [`../../AGENTS.md`](../../AGENTS.md)

## 1. Current Ground Truth

Today Belltower runtime support includes one distinct subscription-backed path, while the auth substrate remains broader than the current runtime set:

- literal secret storage
- environment variable references
- shell command references
- refreshable OAuth token bundles with account metadata in Belltower's own `auth.json` store
- a first distinct subscription-backed login path for `chatgpt` via device-code OAuth

Those credentials live behind the `bt-auth` storage seam and are resolved by
`bt-auth`.
For ChatGPT specifically, Belltower now also has the separate provider/runtime path needed to validate, discover models, refresh tokens, and stream real completions.
Under the foundation plan, `bt-auth` now also has pluggable storage backends
for ephemeral env-var resolution and feature-gated keychain storage, plus
centralized operator-facing backend selection and precedence.

## 2. What We Can Import Safely

Import is only safe when the external credential semantics actually match the Belltower provider semantics.

Safe or likely-safe categories:

- previous Belltower auth store formats
- API keys stored in shell env vars
- API keys exposed by password managers or shell commands
- provider-native API keys stored in external config files, if the file format and meaning are stable

These imports still need validation before Belltower should claim readiness.

## 3. What We Must Not Misrepresent

Do not import or advertise external auth data as a usable Belltower credential when:

- it is for a different product surface
- it is subscription-backed rather than API-billed
- it is scoped to a managed runtime with different permissions
- it only works through a separate CLI or app-server

Examples of risky categories:

- ChatGPT or Codex subscription tokens treated as generic OpenAI API keys
- Claude Code auth treated as Anthropic API billing auth
- Google Cloud local auth treated as a valid Vertex model auth path unless the actual provider implementation supports that exact path

## 4. Future Subscription-Backed Auth

If Belltower later supports subscription-backed or OAuth-backed providers, the rule is:

- create a distinct provider or auth strategy
- store refreshable credentials in Belltower's auth store
- document the runtime boundary clearly
- do not overload existing API-key providers with different semantics

Current example:

- `chatgpt` uses a distinct `openai-chatgpt` provider id plus device-code OAuth login
- the credential can be acquired and refreshed in Belltower's auth store
- the runtime/provider path is implemented separately against the ChatGPT Codex backend rather than overloaded into the generic OpenAI API-key path

That is how `pi` and Hermes keep the UX simple without lying about provider behavior.

## 5. Required Import Behavior

Any auth import path should:

1. detect candidate sources
2. explain what source was found
3. explain what will be imported
4. persist into Belltower's own auth store
5. validate the credential after import
6. report the actual usable state in `belltower doctor` and `belltower status`

Import alone is not success. Successful validation is success.

## 6. Human Acceptance Tests

For every new import path:

1. import a credential
2. run `belltower doctor`
3. run `belltower status --probe`
4. use the provider in a real session
5. remove the credential and verify the provider returns to `missing auth` or equivalent

The operator output must stay truthful before, during, and after the import.
