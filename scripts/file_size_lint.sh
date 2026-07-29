#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
threshold="${BELLTOWER_FILE_SIZE_LIMIT:-1500}"
override_file="$root/docs/development/file-size-overrides.md"
status=0

is_overridden() {
  local rel="$1"
  [[ -f "$override_file" ]] && grep -Fxq "$rel" "$override_file"
}

while IFS= read -r file; do
  rel="${file#"$root"/}"
  if [[ "$rel" == */tests.rs ]]; then
    continue
  fi
  lines="$(wc -l < "$file" | tr -d '[:space:]')"
  if (( lines <= threshold )); then
    continue
  fi
  if is_overridden "$rel"; then
    continue
  fi
  printf 'file exceeds %s lines without override: %s (%s lines)\n' "$threshold" "$rel" "$lines" >&2
  status=1
done < <(
  find "$root/crates" \
    -path '*/tests/*' -prune -o \
    -name '*.rs' -type f -print | sort
)

if [[ -f "$override_file" ]]; then
  while IFS= read -r line; do
    [[ -z "$line" || "$line" =~ ^[[:space:]]*# ]] && continue
    rel="${line%%[[:space:]]*}"
    file="$root/$rel"
    if [[ ! -f "$file" ]]; then
      printf 'file-size override references missing file: %s\n' "$rel" >&2
      status=1
      continue
    fi
    lines="$(wc -l < "$file" | tr -d '[:space:]')"
    if (( lines <= threshold )); then
      printf 'file-size override is stale; file is now within limit: %s (%s lines)\n' "$rel" "$lines" >&2
      status=1
    fi
  done < "$override_file"
fi

if (( status != 0 )); then
  printf 'Update %s only for intentional temporary monoliths.\n' "${override_file#"$root"/}" >&2
fi

exit "$status"
