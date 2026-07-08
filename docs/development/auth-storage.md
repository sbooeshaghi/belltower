# Auth Storage

This document defines the current auth-storage seam for Belltower launcher and
readiness flows.

## Purpose

The auth-storage policy lives in `bt-auth`. Launcher, readiness, and provider
flows consume that policy; they do not reconstruct backend precedence locally.

## Storage Modes

- `auto`
  - default mode when no explicit override is provided
- `file`
  - persist credentials in the managed auth store JSON file
- `keychain`
  - persist credentials in the OS keychain when the build includes the
    `keychain-backend` feature
- `ephemeral`
  - read credentials from environment variables only; writes and deletes fail
    closed

## Selection Rules

Explicit selection precedence:

1. `belltower login --storage ...`
2. `BELLTOWER_AUTH_STORE`
3. default `auto`

`auto` resolution precedence for a given connection:

1. ephemeral environment variables for that connection, when present
2. keychain credential, when available
3. file-store credential as fallback

`auto` write behavior:

- writes go to keychain when available
- otherwise writes go to file storage
- auto mode does not silently migrate an existing file credential into keychain

## Operator Truth

- `belltower doctor`
  - reports the configured auth-storage summary at the top level
  - reports the resolved auth source per connection, including tried backend
    order when auto mode is involved
- `belltower status`
  - reports the same canonical auth-storage summary and per-connection source
    truth

These surfaces are the operator contract. They should describe the actual
backend used to resolve a credential, not a launcher guess and not a hardcoded
file path.
