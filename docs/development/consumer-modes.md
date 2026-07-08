# Consumer Modes

This document defines the intended Belltower consumption seams for different
client and host modes.

The goal is to keep one clean remote-consumer contract while still being honest
about the current launcher-owned local bootstrap path.

## Modes

### Remote Client

Use:

- `bt-protocol` as the authoritative wire schema
- a generated or handwritten remote SDK
- `bt-client::new_with_auth_token(...)` when Rust code wants the built-in client

Rules:

- remote clients should treat Belltower as an authenticated HTTP and SSE server
- remote clients should not depend on endpoint-scoped token file discovery
- remote clients should prefer explicit bearer-token ownership
- remote clients may use the rest of `BelltowerClient` once constructed with
  explicit bearer auth; that is the baseline supported Rust consumer seam
- remote clients should not use the legacy `bt-client::new(...)` alias, because
  it follows the launcher-owned discovered-auth path

### Launcher-Owned Local TUI

Use:

- `belltower` for local server startup and helper resolution
- `bt-client::new_discovering_auth(...)` or `TryFrom<&str>` for the local
  discovered-auth path
- `bt-client::new_with_auth_token_path(...)` /
  `bt-client::with_auth_token_path(...)` when the launcher owns the local token
  file explicitly
- the normal `bt-protocol` routes and SSE stream after bootstrap

Rules:

- local bootstrap remains a launcher concern
- the TUI still consumes the same protocol routes as any other client
- the launcher may own token discovery and helper startup, but the TUI should
  not grow a second protocol path
- filesystem token-path helpers are launcher/local-only; they are not part of
  the remote-client contract
- token-path clients reread the local token file per request so server restarts
  and endpoint token rotation are visible without reconstructing the client

### Local Desktop Host

Current implemented state:

- there is not yet a dedicated host/bootstrap crate
- local desktop bootstrap is currently owned by `belltower`
- hosts that need the current local bootstrap path should treat it as a
  launcher-owned integration seam, not as the baseline remote-client contract

Target direction:

- if Belltower later exposes an explicit host/bootstrap surface, it should stay
  separate from the default remote client contract

### Headless, Editor, Web, and Mobile Consumers

Use:

- `bt-protocol`
- explicit bearer auth
- server metadata and capability reporting through `/server/info`

Rules:

- do not assume launcher behavior, helper binaries, or local filesystem token
  discovery
- capability negotiation should come from protocol surfaces, not route probing

## Design Rules

- `bt-protocol` is the canonical public schema source
- `bt-client` must keep remote auth explicit even when it also supports the
  local discovered-auth path
- crate-private helpers that only support internal client wiring should not stay
  public once a supported consumer path exists
- `belltower` owns launcher/bootstrap behavior until a dedicated host/bootstrap
  seam exists
- control-plane truth should be rendered from canonical inspection surfaces, not
  transcript heuristics
- auth-storage backend selection and precedence should come from `bt-auth`, not
  ad hoc launcher logic

## Practical Guidance

When adding a new client-facing feature, decide first which mode owns it:

- remote client
- launcher-owned local TUI
- future host/bootstrap integration

Then keep the implementation on that seam. Do not silently promote a
launcher-only behavior into the baseline remote client contract.
