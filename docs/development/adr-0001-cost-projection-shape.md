# ADR-0001: Cost Projection Shape

- **Status:** accepted
- **Date:** 2026-04-23
- **Deciders:** Belltower maintainers
- **Supersedes:** none

## Context

Belltower records provider token usage and cost as canonical session
telemetry. `TokenUsage` and `CostBreakdown` already distinguish
prompt, completion, cache-read, cache-write, and reasoning token
classes, but the pricing catalog only represents prompt and completion
rates. The session projection also stores `total_cost_usd` as a
non-null numeric value, which means a true zero-cost completion and an
unpriced completion can currently collapse into the same stored value.

The self-hosting foundation plan requires this contract to be decided
before runtime or store behavior changes. This ADR locks the schema
shape and the reprojection trigger policy for the 1.3 pricing stack so
later tasks can implement the catalog coverage, projection migration,
reprojection harness, and inspection surfaces without re-litigating the
meaning of stored cost.

## Options considered

1. **Nullable total cost.** Change `total_cost_usd` to nullable and use
   `NULL` for unpriced completions. This directly represents unknown
   cost, but it weakens existing consumers that expect a numeric total
   and forces every aggregate query to branch on nullability.

2. **Non-null total plus unpriced counter.** Retain
   `total_cost_usd REAL NOT NULL DEFAULT 0` and add an
   `unpriced_completion_count` projection field. A true zero-cost
   completion remains numeric zero with no unpriced increment; an
   unpriced completion increments the counter. This keeps current
   numeric aggregates simple while making unpriced history explicit.

3. **Dual priced/unpriced counters.** Store separate priced counts,
   unpriced counts, and potentially multiple partial totals by token
   class. This is the richest representation, but it adds schema and
   projection complexity before Belltower has enough cost consumers to
   justify it.

4. **Flat token-class pricing fields.** Extend pricing entries with
   optional `cache_read_usd_per_million`,
   `cache_write_usd_per_million`, and `reasoning_usd_per_million`
   fields. This matches the existing `TokenUsage` and `CostBreakdown`
   shape and keeps TOML readable. It is less generic than a token-class
   map, but avoids creating a second mini schema for a small, known set
   of token classes.

5. **Token-class map.** Store token-class rates in a map keyed by enum
   or string. This is extensible, but harder to validate in TOML and
   easier for providers to drift from the fixed usage fields Belltower
   already exposes.

6. **Operator-initiated reprojection.** Existing session costs are
   recomputed only when an operator explicitly runs a future slash
   command or CLI subcommand. Catalog changes do not silently mutate
   history. This is the safest default for auditable telemetry.

7. **Automatic reprojection on catalog-hash change.** Store a pricing
   catalog content hash and automatically repair historical sessions on
   startup or in the background when the catalog changes. This keeps
   old sessions current, but silently mutates historical cost surfaces
   unless every change is carefully recorded and surfaced.

8. **Manual plus audit-only.** Detect catalog drift and report that
   sessions can be reprojected, but never provide a built-in repair
   operation. This preserves history but leaves operators without a
   practical way to correct known incomplete costs.

## Decision

Belltower will use Option 2 for the projection schema: keep
`total_cost_usd REAL NOT NULL DEFAULT 0` and add an
`unpriced_completion_count` projection field in the store migration
slice. Belltower will use Option 4 for pricing data: pricing shapes
gain optional cache-read, cache-write, and reasoning token-class rates,
`ModelPricing` can carry an explicit `unpriced_reason`, and the catalog
gains explicit unpriced model entries with reason strings. Belltower
will use Option 6 for reprojection: historical cost backfill is
operator-initiated only, through the reusable event-log walk seam added
later in the 1.3 stack.

## Consequences

- Positive: Existing numeric cost totals remain easy to aggregate.
- Positive: Unpriced completions become visible instead of being
  indistinguishable from free completions.
- Positive: The pricing catalog can model current provider token
  classes without introducing a generic token-pricing DSL.
- Positive: Reprojection is explicit, auditable, and controlled by the
  operator rather than a hidden startup side effect.
- Negative: Token classes outside the current prompt/completion/cache
  and reasoning set require another schema decision.
- Negative: Operators must run the future reprojection command when
  they want historical sessions repaired after pricing catalog changes.
- Follow-on work: 1.3b populates priced and explicitly unpriced catalog
  rows; 1.3c adds the projection field and writer behavior; 1.3d adds
  the reusable `bt-session` reprojection harness; 1.3e exposes cost
  summary fields in inspection/runtime consumers.

## Invariants preserved

- IR-12: cost surfaces stay honest as provider pricing coverage evolves.
- Maintainability Principle 5: data shapes are defined before runtime
  behavior and store projection changes.
- Maintainability Principle 7: cost telemetry remains reconstructable
  from canonical session state.

## Invariants at risk

The main risk is semantic drift between catalog entries, runtime cost
math, and projection behavior. The mitigation is the serial 1.3 stack:
no runtime or session behavior changes land until the ADR and
`bt-core` pricing shapes are committed, and later slices add coverage
tests at the catalog, projection, reprojection, and inspection seams.
