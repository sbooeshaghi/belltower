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
# Keep the pure agent-loop boundary intact; extract lifecycle phases once the
# observer/approval seams are stable enough to name independently.
crates/bt-agent/src/lib.rs
# Prompt assembly and compaction policy are still evolving together; split
# public assembly, retention policy, and tests in a dedicated structural pass.
crates/bt-context/src/lib.rs
# Embedded catalog parsing and validation are only slightly over the limit;
# extract persistence helpers when that ownership seam grows.
crates/bt-core/src/config.rs
crates/bt-core/src/types.rs
crates/bt-readiness/src/lib.rs
crates/bt-server/src/main.rs
# Export, validation, import, and diff need a dedicated ownership-based split,
# not a cosmetic move mixed into compaction correctness work.
crates/bt-session/src/export.rs
crates/bt-session/src/store.rs
