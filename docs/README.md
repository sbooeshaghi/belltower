# Belltower Docs

This directory is organized by purpose:

- `architecture/`
  - high-level design and architectural commitments
- `planning/`
  - implementation targets, parity analysis, and scope management
- `development/`
  - repo/process guidance and development patterns
- `subsystems/`
  - focused subsystem docs for the harness itself
- `api/`
  - generated OpenAPI contract for client consumers

If you are starting work on Belltower, read these first:

1. [`architecture/overview.md`](./architecture/overview.md)
2. [`architecture/event-taxonomy-and-write-path.md`](./architecture/event-taxonomy-and-write-path.md)
3. [`architecture/provider-output-and-transcript-model.md`](./architecture/provider-output-and-transcript-model.md)
4. [`architecture/tool-system-and-normalization.md`](./architecture/tool-system-and-normalization.md)
5. [`architecture/tool-policy.md`](./architecture/tool-policy.md)
6. [`architecture/signals-and-ingress.md`](./architecture/signals-and-ingress.md)
7. [`architecture/reactions-and-playbooks.md`](./architecture/reactions-and-playbooks.md)
8. [`architecture/memory.md`](./architecture/memory.md)
9. [`architecture/memory-subsystem-spec.md`](./architecture/memory-subsystem-spec.md)
10. [`architecture/hooks-and-lifecycle.md`](./architecture/hooks-and-lifecycle.md)
11. [`architecture/participants-and-collaboration.md`](./architecture/participants-and-collaboration.md)
12. [`architecture/skills-and-self-improvement.md`](./architecture/skills-and-self-improvement.md)
13. [`architecture/claude-code-lessons-and-adoption.md`](./architecture/claude-code-lessons-and-adoption.md)
14. [`architecture/subagents-and-session-graphs.md`](./architecture/subagents-and-session-graphs.md)
15. [`architecture/session-bundles-and-trace-sync.md`](./architecture/session-bundles-and-trace-sync.md)
16. [`architecture/datasets-and-training-artifacts.md`](./architecture/datasets-and-training-artifacts.md)
17. [`planning/target-implementation-goals.md`](./planning/target-implementation-goals.md)
18. [`planning/platform-roadmap.md`](./planning/platform-roadmap.md)
19. [`planning/current-sprint.md`](./planning/current-sprint.md)
20. [`planning/subagent-rollout.md`](./planning/subagent-rollout.md)
21. [`planning/scientific-workflow-foundation-plan.md`](./planning/scientific-workflow-foundation-plan.md)
22. [`planning/subsystem-parity-matrix.md`](./planning/subsystem-parity-matrix.md)
23. [`planning/case-study-review.md`](./planning/case-study-review.md)
24. [`planning/harness-landscape-review-20260408.md`](./planning/harness-landscape-review-20260408.md)
25. [`planning/pre-improvement-subsystem-review-20260408.md`](./planning/pre-improvement-subsystem-review-20260408.md)
26. [`planning/pre-merge-review-20260410.md`](./planning/pre-merge-review-20260410.md)
27. [`development/pattern-catalog.md`](./development/pattern-catalog.md)

Development docs that support implementation work:

- [`../AGENTS.md`](../AGENTS.md)
- [`install.md`](./install.md)
- [`api/README.md`](./api/README.md)
- [`development/source-of-truth-matrix.md`](./development/source-of-truth-matrix.md)
- [`development/developer-guidelines.md`](./development/developer-guidelines.md)
- [`development/coherence-audit-20260331.md`](./development/coherence-audit-20260331.md)
- [`development/pre-friend-testing-audit-20260401.md`](./development/pre-friend-testing-audit-20260401.md)
- [`development/next-agent-handoff-20260403.md`](./development/next-agent-handoff-20260403.md)
- [`development/tui-functional-requirements.md`](./development/tui-functional-requirements.md)
- [`development/tui-reference-review-20260402.md`](./development/tui-reference-review-20260402.md)
- [`development/protocol-stabilization.md`](./development/protocol-stabilization.md)
- [`development/consumer-modes.md`](./development/consumer-modes.md)
- [`development/provider-integration-checklist.md`](./development/provider-integration-checklist.md)
- [`development/tui-testing.md`](./development/tui-testing.md)
- [`development/auth-migration-and-imports.md`](./development/auth-migration-and-imports.md)

Then read the relevant subsystem docs:

- [`subsystems/core-protocol-and-server.md`](./subsystems/core-protocol-and-server.md)
- [`subsystems/session-runtime-and-agent.md`](./subsystems/session-runtime-and-agent.md)
- [`subsystems/providers-auth-and-models.md`](./subsystems/providers-auth-and-models.md)
- [`subsystems/tools-context-and-approvals.md`](./subsystems/tools-context-and-approvals.md)
- [`subsystems/launcher-and-tui.md`](./subsystems/launcher-and-tui.md)
- [`subsystems/tui-architecture.md`](./subsystems/tui-architecture.md)
- [`subsystems/tui-cutover-spec.md`](./subsystems/tui-cutover-spec.md)
- [`subsystems/mcp.md`](./subsystems/mcp.md)
- [`subsystems/telemetry-and-exports.md`](./subsystems/telemetry-and-exports.md)

These docs are intentionally separated:

- architecture docs explain what Belltower is
- planning docs explain what we still need to implement
- development docs explain how we should work on it
- subsystem docs explain how the major parts fit together

When these sources disagree, use [`development/source-of-truth-matrix.md`](./development/source-of-truth-matrix.md).
