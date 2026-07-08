#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/publish.sh [--preflight|--dry-run|--publish|--install-check]

  --preflight      Bootstrap-safe packaging preflight for CI before the
                   internal crates exist on crates.io.
  --dry-run        Run cargo publish --dry-run in dependency order.
                   Requires already-published internal dependencies except
                   for the first crate in a fresh bootstrap release.
  --publish        Publish crates in dependency order.
  --install-check  Install belltower, bt-server, and bt-tui from local paths
                   into a temporary Cargo root and verify the entrypoint.

The primary operator command is `belltower`, but the current package boundary
keeps the server and TUI helpers as sibling binary crates. Until those helpers
become libraries inside the belltower package, a working cargo-installed setup
must install all three binary packages.
EOF
}

mode="${1:---preflight}"
case "$mode" in
  --preflight|--dry-run|--publish|--install-check)
    ;;
  -h|--help)
    usage
    exit 0
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac

workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$workspace_root"

publish_order=(
  bt-core
  bt-tools
  bt-context
  bt-auth
  bt-models
  bt-mcp
  bt-otel
  bt-providers
  bt-agent
  bt-session
  bt-protocol
  bt-client
  bt-readiness
  bt-runtime
  bt-server
  bt-tui
  bt-improve
  belltower
)

if [[ "$mode" == "--install-check" ]]; then
  install_root="${BT_INSTALL_CHECK_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/belltower-install-check.XXXXXX")}"
  cargo install --offline --locked --path crates/bt-server --bin bt-server --root "$install_root"
  cargo install --offline --locked --path crates/bt-tui --bin bt-tui --root "$install_root"
  cargo install --offline --locked --path crates/belltower --bin belltower --root "$install_root"
  "$install_root/bin/belltower" --version
  echo "Installed Belltower binaries under $install_root/bin"
  exit 0
fi

if [[ "$mode" == "--preflight" ]]; then
  cargo metadata --no-deps --format-version 1 > /dev/null
  package_flags=()
  publish_flags=(--locked -p bt-core)
  if [[ "${BT_PUBLISH_ALLOW_DIRTY:-0}" == "1" ]]; then
    package_flags+=(--allow-dirty)
    publish_flags+=(--allow-dirty)
  fi
  for crate in "${publish_order[@]}"; do
    cargo package --list -p "$crate" "${package_flags[@]}" > /dev/null
  done
  # Full publish verification is only possible for bootstrap root crates
  # until downstream bt-* versions exist in the registry.
  cargo publish --dry-run "${publish_flags[@]}"
  exit 0
fi

for crate in "${publish_order[@]}"; do
  publish_flags=(--locked -p "$crate")
  if [[ "${BT_PUBLISH_ALLOW_DIRTY:-0}" == "1" ]]; then
    publish_flags+=(--allow-dirty)
  fi
  if [[ "$mode" == "--dry-run" ]]; then
    cargo publish --dry-run "${publish_flags[@]}"
  else
    cargo publish "${publish_flags[@]}"
  fi
done
