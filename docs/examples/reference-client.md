# `bt-client` Reference Consumer

This example demonstrates the supported Rust remote-client seam:

- explicit bearer-token ownership through `BelltowerClient::new_with_auth_token`
- `/server/info` capability negotiation before creating a session
- session creation and message submission through protocol DTOs
- SSE event streaming through `stream_events`
- approval resolution through the same protocol route a UI would use
- JSONL export through the public export route

It intentionally does **not** use launcher-owned local token discovery. Remote
clients should own the server URL and bearer token explicitly.

## Run

Start Belltower in another terminal:

```bash
cargo run -p belltower --
```

Set an explicit bearer token for that server, then run the example:

```bash
export BELLTOWER_URL=http://127.0.0.1:7400
export BELLTOWER_AUTH_TOKEN=<server bearer token>
cargo run -p bt-client --example reference_client
```

For the default local server URL, the launcher/server writes the endpoint token
to `~/.config/belltower/server-auth/http_127_0_0_1_7400.token`. Passing that
token through `BELLTOWER_AUTH_TOKEN` keeps this example on the remote-consumer
path instead of using launcher-only token discovery.

Optional knobs:

```bash
export BELLTOWER_REFERENCE_CONNECTION=local
export BELLTOWER_REFERENCE_MODEL=qwen3.5:latest
export BELLTOWER_REFERENCE_PROJECT_ROOT="$PWD"
export BELLTOWER_REFERENCE_PROMPT="Say hello from the reference client."
```

The example prints streamed assistant text as it arrives, approves any approval
request once if one appears, waits for `turn.finished`, then exports the session
as JSONL and prints the export size.
