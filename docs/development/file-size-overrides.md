# File Size Overrides
#
# Non-test Rust source files should stay under 1,500 lines. This file is
# the explicit exception list consumed by `scripts/file_size_lint.sh`.
#
# Overrides are temporary coordination records, not permission to grow
# the files further. Remove an entry once the file is split or shrunk.

# Current intentional exceptions:
#
# These files remain active subsystem roots or dense shared type surfaces.
# Split them only when a real ownership seam emerges; do not introduce
# cosmetic modules solely to satisfy the line-count heuristic.
crates/belltower/src/main.rs
crates/bt-core/src/types.rs
crates/bt-readiness/src/lib.rs
crates/bt-server/src/main.rs
crates/bt-session/src/store.rs
