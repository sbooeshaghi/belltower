# ADR Template

Copy this file to `docs/development/adr-NNNN-<short-slug>.md` when
recording a new architectural decision. Numbering is monotonic per
repo; check the existing `adr-*.md` list for the next free number.

Keep ADRs short. If the decision needs more than ~2 pages of
explanation, the decision is probably wrong or incomplete.

---

# ADR-NNNN: <Decision title>

- **Status:** proposed | accepted | superseded by ADR-XXXX
- **Date:** YYYY-MM-DD
- **Deciders:** <names or handles>
- **Supersedes:** <ADR-XXXX if any>

## Context

Two or three paragraphs. What is the problem? What constraints apply?
What forces are in play? Cite the architecture doc, subsystem doc, or
foundation plan section that made this decision necessary.

## Options considered

Numbered list of options. For each:

1. **<Option name>.** One paragraph describing the option. One or two
   sentences on its consequences. One sentence on why it was or was
   not chosen.

## Decision

One paragraph. State the chosen option unambiguously. If the decision
has sub-parts (e.g., "schema shape X, reprojection trigger Y"), state
each sub-part.

## Consequences

Bulleted list. What changes as a result? What becomes easier? What
becomes harder? What migrations or follow-ons does this decision
imply?

- Positive: …
- Negative: …
- Follow-on work: …

## Invariants preserved

Which architectural invariants this decision protects (cite by FR/IR
number or Maintainability Principle number from the foundation plan).

## Invariants at risk

Any invariants this decision stresses or weakens, and how the plan
mitigates the stress.
