#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
status=0

while IFS=: read -r manifest line text; do
  path="$(printf '%s\n' "$text" | sed -n 's/.*path[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p')"
  case "$path" in
    ../bt-*)
      ;;
    *)
      printf 'disallowed path dependency in %s:%s: %s\n' "$manifest" "$line" "$text" >&2
      status=1
      ;;
  esac
done < <(
  awk '
    /^\[.*dependencies.*\]$/ { in_dependencies = 1; next }
    /^\[/ { in_dependencies = 0; next }
    in_dependencies && /path[[:space:]]*=[[:space:]]*"[^"]*"/ {
      printf "%s:%d:%s\n", FILENAME, FNR, $0
    }
  ' "$root"/crates/*/Cargo.toml || true
)

if grep -RInE '^[[:space:]]*(frollo|carillon|semaphora|derivis)[-_A-Za-z0-9]*[[:space:]]*=' "$root"/crates/*/Cargo.toml; then
  printf 'app-specific crate dependency detected in Belltower crate manifests\n' >&2
  status=1
fi

if [[ "$status" -ne 0 ]]; then
  printf 'Belltower crates must remain independent of app-specific workspace crates and services.\n' >&2
fi

exit "$status"
