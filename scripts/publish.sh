#!/usr/bin/env bash
set -euo pipefail

cat >&2 <<'EOF'
scripts/publish.sh is retained as a compatibility entrypoint only.
Belltower 0.1 does not publish workspace crates to crates.io; use
scripts/distribution.sh for source/archive distribution checks.
EOF

exec bash "$(dirname "${BASH_SOURCE[0]}")/distribution.sh" "$@"
