# Spoke Schemas: Observed Session Formats of Foreign Harnesses

Belltower is the hub; other agent harnesses are spokes
([plan](../../docs/planning/harness-interop-plan-20260725.md)). Each file in
this directory is the **defined truth** for one spoke's on-disk session
format, and the contract the corresponding Belltower ingress/egress adapter
is built and tested against.

These schemas are **empirical, not aspirational**. None of these formats are
ours and none are publicly specified; each schema was derived post-hoc by
surveying the real session corpus on an operator machine plus fresh capture
runs, and describes exactly what was observed. The companion `*.notes.md`
records provenance (corpus size, harness versions, capture dates), the
linkage invariants an importer relies on, known quirks and format drift, and
which records are classified as skippable bookkeeping noise.

Doctrine — irreducibly simple but actually compatible:

- **Strictness lives in `required`, not closure.** Load-bearing fields (type
  discriminators, ids, linkage keys, timestamps) are required and strictly
  typed; every object allows `additionalProperties: true` because the
  vendors evolve these formats and Belltower must degrade gracefully rather
  than reject tomorrow's lines.
- **Nothing invented.** Every property in a spoke schema was observed in a
  real corpus or a controlled capture. Fields with multiple observed types
  are unions, called out in the notes.
- **Observed versions are pinned in the notes**, and a schema revision bumps
  only when the *contract an adapter relies on* changes — new optional
  vendor fields do not require a revision.
- **Privacy:** schemas and notes contain synthesized structural examples
  only; never real transcript content.

Current spokes:

| spoke | schema | store surveyed |
|---|---|---|
| Claude Code | `claude-code-session.v1.schema.json` | `~/.claude/projects/<cwd-slug>/<id>.jsonl` + `<id>/` sidecars |
| Codex CLI | `codex-session.v1.schema.json` | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` + `session_index.jsonl` |
| Hermes Agent | `hermes-session.v1.schema.json` | `~/.hermes/sessions/session_*.json` (single JSON document) |

The hub's own schemas (`belltower-event-v1`, `belltower-bundle-manifest-v1`,
schemars-generated with a CI drift gate) are Phase 0 of the interop plan and
live in `schema/` proper once generated; spokes validate against *their*
schema on ingress, and every synthesized canonical log must pass the hub's
validator before an import commits.
