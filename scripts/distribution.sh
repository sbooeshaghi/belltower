#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/distribution.sh [--preflight|--install-check]

Belltower 0.1 is distributed as GitHub release archives or from a source
checkout. The workspace crates are intentionally not published to crates.io.

  --preflight      Verify the source/archive distribution contract for CI.
  --install-check  Install the three binaries from local source paths into a
                   temporary Cargo root and verify launcher helper discovery.
EOF
}

mode="${1:---preflight}"
case "$mode" in
  --preflight|--install-check)
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

check_source_distribution_contract() {
  local manifest package

  test -f LICENSE
  cargo metadata --no-deps --format-version 1 > /dev/null

  for manifest in crates/*/Cargo.toml; do
    package="$(sed -n 's/^name = "\([^"]*\)"/\1/p' "$manifest" | head -n 1)"
    if ! grep -Fqx 'publish = false' "$manifest"; then
      echo "source-only distribution error: $package must set publish = false" >&2
      return 1
    fi
  done

  grep -Fqx 'name = "belltower"' crates/belltower/Cargo.toml
  grep -Fqx 'name = "bt-server"' crates/bt-server/Cargo.toml
  grep -Fqx 'name = "bt-tui"' crates/bt-tui/Cargo.toml
  check_binary_target crates/belltower/Cargo.toml belltower
  check_binary_target crates/bt-server/Cargo.toml bt-server
  check_binary_target crates/bt-tui/Cargo.toml bt-tui
}

check_binary_target() {
  local manifest="$1" binary="$2"

  if ! awk -v expected="$binary" '
    /^\[\[bin\]\]$/ { in_bin = 1; next }
    in_bin && /^\[/ { in_bin = 0 }
    in_bin && $0 == "name = \"" expected "\"" { found = 1 }
    END { exit(found ? 0 : 1) }
  ' "$manifest"; then
    echo "source-only distribution error: $manifest must declare [[bin]] $binary" >&2
    return 1
  fi
}

check_source_distribution_contract

if [[ "$mode" == "--preflight" ]]; then
  echo "Belltower 0.1 source/archive distribution preflight passed."
  exit 0
fi

install_root="${BT_INSTALL_CHECK_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/belltower-install-check.XXXXXX")}"
install_target="${BT_INSTALL_CHECK_TARGET_DIR:-$install_root/target}"
CARGO_TARGET_DIR="$install_target" cargo install --offline --locked --path crates/bt-server --bin bt-server --root "$install_root"
CARGO_TARGET_DIR="$install_target" cargo install --offline --locked --path crates/bt-tui --bin bt-tui --root "$install_root"
CARGO_TARGET_DIR="$install_target" cargo install --offline --locked --path crates/belltower --bin belltower --root "$install_root"
"$install_root/bin/belltower" --version
doctor_output="$(BELLTOWER_CONFIG_DIR="$install_root/config" "$install_root/bin/belltower" doctor)"
grep -Fq "[ok] server binary:" <<<"$doctor_output"
grep -Fq "[ok] tui binary:" <<<"$doctor_output"
echo "Installed Belltower binaries under $install_root/bin"
