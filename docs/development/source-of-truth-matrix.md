# Documentation Source-of-Truth Matrix

This document defines how Belltower resolves doc/code conflicts during active development.

The goal is not to make every doc equally authoritative.
The goal is to make it obvious where truth lives for each kind of claim.

## Truth Tiers

| Surface | Truth tier | Normative for | Not normative for |
| --- | --- | --- | --- |
| Code and focused tests | Current behavior | Implemented commands, routes, DTOs, tool names, provider behavior, stored fields, visible operator behavior | Intended future architecture |
| `docs/architecture/` | Normative architecture | Invariants, boundaries, crate ownership, canonical concepts, long-lived design commitments | Exact current command/route inventory unless stated explicitly |
| `docs/subsystems/` | Normative current structure | Current subsystem behavior, major operator flows, public surface descriptions, current implementation boundaries | Future sequencing and aspirational roadmap claims |
| `docs/planning/target-implementation-goals.md` | Normative target bar | Release requirements, product bar, stable obligations sequencing must respect | Claims about what is already implemented |
| `docs/planning/current-sprint.md` | Active planning | Current phase, remaining release-critical work, implementation focus | Exact current behavior unless explicitly marked complete |
| `docs/planning/platform-roadmap.md` | Sequencing | Ordered future work after the current sprint | Current implementation truth |
| `docs/planning/subsystem-parity-matrix.md` | Planning/reference hybrid | Package map, subsystem coverage, human test matrix, near-term planning context | Exact current “next steps” after the codebase has moved on |
| `docs/development/` | Developer workflow | How to work on Belltower, test it, and reason about repo process | Product or runtime truth unless directly derived from code |
| Dated planning and audit reviews such as `docs/planning/case-study-review.md`, `docs/planning/harness-landscape-review-20260408.md`, `docs/planning/pre-improvement-subsystem-review-20260408.md`, and `docs/planning/pre-merge-review-20260410.md` | Reference only | Comparative analysis, historical audit context, and pre-improvement rationale | Current Belltower behavior or active release commitments |
| `docs/human_notes/` | Reference only | Historical thinking and design discussion | Current product truth |
| `docs/README.md` | Navigation | Reader orientation and truth-tier guidance | Product/runtime claims beyond directory guidance |

## Conflict Resolution Rules

When two sources disagree, use these rules in order:

1. If code violates an explicit architectural invariant, fix the code or revise the invariant explicitly.
2. If docs describe future or intended behavior as already implemented, fix the docs.
3. If multiple docs disagree, align the lower-tier docs to the higher-tier source.
4. If the architecture docs are ambiguous, make one explicit decision in `docs/architecture/` first, then propagate it outward.
5. Do not preserve transitional wording once the transition is over.

## Practical Audit Surfaces

During coherence passes, treat these as the primary comparison surfaces:

- launcher and TUI operator commands
- protocol routes, DTOs, and SSE semantics
- event taxonomy and transcript/provider-output model
- tool inventory, tool modes, and approval behavior
- provider, auth, model, and readiness behavior
- MCP lifecycle and operator surfaces
- session, branch, workflow, and raw-chunk inspection
- atomic user ingress, active-turn ownership, and queued/control continuation
  admission
- telemetry and export behavior

## Expected Outcomes

A successful alignment pass leaves Belltower with:

- one clear architectural description per major subsystem
- subsystem docs that match current code
- planning docs that describe remaining work rather than old work
- development docs that match current workflows and commands
- reference docs clearly treated as reference material, not live truth
