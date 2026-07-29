# Installing Belltower

Belltower's user entrypoint is `belltower`. The current runtime also
needs the `bt-server` and `bt-tui` helper binaries available next to
`belltower` or somewhere on `PATH`.

## Option 1: Cargo From A Source Checkout

Use this when you have a Rust toolchain:

```bash
cargo install --path crates/bt-server
cargo install --path crates/bt-tui
cargo install --path crates/belltower
belltower --version
belltower doctor
```

Run those commands from the Belltower repository root. They install the helper
binaries and launcher into Cargo's bin directory. Ensure that directory is on
`PATH`; Cargo usually prints it after installation.

Version 0.1 deliberately does not publish workspace crates to crates.io.
Use a GitHub release archive or this source-checkout path instead. The
workspace crates are implementation details rather than a public Rust crate
API; this also avoids conflating Belltower with unrelated registry packages
that already use the `belltower` and `bt-runtime` names.

## Option 2: Release Archive

Use this when you do not want to build from source:

1. Download the archive for your platform from GitHub Releases.
2. Unpack it into a directory such as `~/bin/belltower`.
3. Add that directory to `PATH`.
4. Keep `belltower`, `bt-server`, and `bt-tui` together in that
   directory.

On macOS, if Gatekeeper marks the downloaded binaries as quarantined,
remove the quarantine attribute after you have verified the download:

```bash
xattr -dr com.apple.quarantine ~/bin/belltower
```

## Development From A Source Checkout

Use this when developing Belltower:

```bash
git clone https://github.com/sbooeshaghi/belltower.git
cd belltower
cargo run -p belltower --
```

For a release-style local build:

```bash
cargo build --release -p belltower -p bt-server -p bt-tui --bins
target/release/belltower --version
```

If you copy the release binaries elsewhere, copy all three binaries:
`belltower`, `bt-server`, and `bt-tui`.

## Verify Your Install

Run:

```bash
belltower --version
belltower doctor
belltower status
```

Expected result:

- `belltower --version` prints the installed version.
- `belltower doctor` prints structured connection and auth checks.
- `belltower status` prints the configured defaults and readiness
  state.

Missing auth or a degraded provider is not an install failure. It means
you still need to run setup or login for the provider you want to use:

```bash
belltower setup
belltower login chatgpt
```

## Headless Use

Use `belltower run` for scripts, benchmarks, and non-interactive checks:

```bash
belltower run --prompt "Summarize this repository"
printf 'Write a short test plan for this repo\n' | belltower run
```

This path still auto-starts `bt-server`, creates a normal Belltower
session, sends the prompt through `bt-client`, and records the same
runtime/session telemetry as the TUI. Use `--json` when a caller needs
the session id, branch id, selected connection/model, and assistant
text as structured output.

## Troubleshooting

- `could not resolve bt-server` or `could not resolve bt-tui`: install the
  helper binaries from a source checkout using the three `cargo install
  --path` commands above, or keep all three release-archive binaries in the
  same directory.
- `belltower doctor` reports missing auth: run `belltower login
  <connection>` for the connection you want to use.
- The local server port is busy: stop the existing Belltower server, or
  update the server port in `~/.config/belltower/config.toml`.
- Local model readiness fails: start your local OpenAI-compatible
  backend, or switch providers with `belltower setup` and the TUI
  `/use` command.
